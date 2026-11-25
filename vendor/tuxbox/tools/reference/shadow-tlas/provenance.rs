//! Submission-order provenance model for a future GPU-visible BLAS directory.
//!
//! This is an offline correctness control, not a runtime implementation. It
//! models only recorded deserialize writes, reads, and already-enqueued fence
//! signals/waits. It rejects unordered conflicting accesses and unknown waits.
//! Actual runtime use additionally needs resource ownership, GPU publication,
//! actual resource ownership, GPU publication, full alias tracking, and
//! submission failure handling. Retirement here models verified GPU fence
//! completion, but does not itself wait for or own native fences.
use std::collections::{BTreeMap, VecDeque};

type Queue = u32;
type Fence = u32;
type Address = u64;
type Version = u64;
type Clock = BTreeMap<Queue, u64>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Error {
    UnknownQueue,
    UnknownFence,
    FenceOrder,
    UnorderedAccess,
    UnknownAddress,
    Unsupported,
    UnknownSource,
    InvalidRange,
    Alias,
}

fn before(a: &Clock, b: &Clock) -> bool {
    a.iter()
        .all(|(queue, value)| *value <= *b.get(queue).unwrap_or(&0))
}

#[derive(Clone)]
struct Write {
    clock: Clock,
    version: Version,
    resource: u64,
}
#[derive(Clone)]
struct Resource {
    active: bool,
    uses: Vec<Clock>,
}
#[derive(Clone, Default)]
struct Entry {
    writer: Option<Write>,
    readers: Vec<Clock>,
}
#[derive(Clone, Default)]
struct Graph {
    queues: BTreeMap<Queue, Clock>,
    fences: BTreeMap<Fence, BTreeMap<u64, Clock>>,
    addresses: BTreeMap<Address, Entry>,
    sources: BTreeMap<u64, Version>,
    ranges: BTreeMap<Address, u64>,
    resources: BTreeMap<u64, Resource>,
    next_resource: u64,
    completed: Clock,
}

#[derive(Clone, Copy)]
enum Op {
    Deserialize(Address, Version),
    DeserializeFrom(Address, u64),
    Read(Address),
    Unsupported,
}

impl Graph {
    fn add_queue(&mut self, queue: Queue) {
        self.queues.entry(queue).or_default();
    }

    // Exact base-address lookups only. Reject overlapping extents rather than
    // guessing which placed resource or suballocation owns the address.
    fn register(&mut self, start: Address, bytes: u64) -> Result<(), Error> {
        if start == 0 || start % 256 != 0 || bytes == 0 {
            return Err(Error::InvalidRange);
        }
        let end = start.checked_add(bytes).ok_or(Error::InvalidRange)?;
        if self
            .ranges
            .iter()
            .any(|(&other, &other_end)| start < other_end && other < end)
        {
            return Err(Error::Alias);
        }
        self.ranges.insert(start, end);
        Ok(())
    }

    fn tick(&mut self, queue: Queue) -> Result<Clock, Error> {
        let clock = self.queues.get_mut(&queue).ok_or(Error::UnknownQueue)?;
        let value = clock.entry(queue).or_default();
        *value = value.checked_add(1).ok_or(Error::Unsupported)?;
        Ok(clock.clone())
    }

    // Treat a batch as one validation transaction: failed publication does not
    // leave part of a rejected list visible in the proof model.
    fn submit(&mut self, queue: Queue, ops: &[Op]) -> Result<Vec<Version>, Error> {
        let mut staged = self.clone();
        let mut reads = Vec::new();
        for op in ops {
            let op = match *op {
                Op::DeserializeFrom(address, source) => Op::Deserialize(
                    address,
                    *staged.sources.get(&source).ok_or(Error::UnknownSource)?,
                ),
                other => other,
            };
            let clock = staged.tick(queue)?;
            match op {
                Op::Deserialize(address, version) => {
                    if !staged.ranges.contains_key(&address) {
                        return Err(Error::UnknownAddress);
                    }
                    let entry = staged.addresses.entry(address).or_default();
                    if entry
                        .writer
                        .as_ref()
                        .is_some_and(|v| !before(&v.clock, &clock))
                        || entry.readers.iter().any(|reader| !before(reader, &clock))
                    {
                        return Err(Error::UnorderedAccess);
                    }
                    let resource = staged
                        .next_resource
                        .checked_add(1)
                        .ok_or(Error::Unsupported)?;
                    staged.next_resource = resource;
                    if let Some(previous) = &entry.writer {
                        staged
                            .resources
                            .get_mut(&previous.resource)
                            .ok_or(Error::Unsupported)?
                            .active = false;
                    }
                    staged.resources.insert(
                        resource,
                        Resource {
                            active: true,
                            uses: vec![clock.clone()],
                        },
                    );
                    entry.writer = Some(Write {
                        clock,
                        version,
                        resource,
                    });
                    entry.readers.clear();
                }
                Op::Read(address) => {
                    let entry = staged
                        .addresses
                        .get_mut(&address)
                        .ok_or(Error::UnknownAddress)?;
                    let writer = entry.writer.as_ref().ok_or(Error::UnknownAddress)?;
                    if !before(&writer.clock, &clock) {
                        return Err(Error::UnorderedAccess);
                    }
                    reads.push(writer.version);
                    staged
                        .resources
                        .get_mut(&writer.resource)
                        .ok_or(Error::Unsupported)?
                        .uses
                        .push(clock.clone());
                    entry.readers.push(clock);
                }
                Op::Unsupported => return Err(Error::Unsupported),
                Op::DeserializeFrom(_, _) => unreachable!(),
            }
        }
        *self = staged;
        Ok(reads)
    }

    // A signal is enqueued after all preceding queue operations; it need not yet
    // have completed on the GPU. Only monotonically increasing values are in
    // this bounded model. Fences with future/unknown signals are unsupported.
    fn signal(&mut self, queue: Queue, fence: Fence, value: u64) -> Result<(), Error> {
        if self
            .fences
            .get(&fence)
            .and_then(|f| f.last_key_value())
            .is_some_and(|(&last, _)| value <= last)
        {
            return Err(Error::FenceOrder);
        }
        let clock = self.tick(queue)?;
        self.fences.entry(fence).or_default().insert(value, clock);
        Ok(())
    }

    // Caller must have observed the specified signal complete on the GPU.
    // Merge its full dependency clock: an executed queue wait guarantees the
    // signaled predecessors also finished. Unknown values are never guessed.
    fn complete(&mut self, fence: Fence, value: u64) -> Result<(), Error> {
        let clock = self
            .fences
            .get(&fence)
            .and_then(|f| f.get(&value))
            .ok_or(Error::UnknownFence)?;
        for (&queue, &tick) in clock {
            self.completed
                .entry(queue)
                .and_modify(|v| *v = (*v).max(tick))
                .or_insert(tick);
        }
        Ok(())
    }

    fn collect(&mut self) -> Vec<u64> {
        let retired: Vec<_> = self
            .resources
            .iter()
            .filter_map(|(&id, resource)| {
                (!resource.active
                    && resource
                        .uses
                        .iter()
                        .all(|clock| before(clock, &self.completed)))
                .then_some(id)
            })
            .collect();
        for id in &retired {
            self.resources.remove(id);
        }
        retired
    }

    fn wait(&mut self, queue: Queue, fence: Fence, value: u64) -> Result<(), Error> {
        let published = self
            .fences
            .get(&fence)
            .and_then(|f| f.range(value..).next())
            .map(|(_, c)| c.clone())
            .ok_or(Error::UnknownFence)?;
        let current = self.queues.get_mut(&queue).ok_or(Error::UnknownQueue)?;
        for (source, tick) in published {
            current
                .entry(source)
                .and_modify(|v| *v = (*v).max(tick))
                .or_insert(tick);
        }
        self.tick(queue)?;
        Ok(())
    }
}

// An isolated topological scheduler for queue operations, including a wait
// recorded before its future signal. Pending operations remain queued rather
// than being speculatively published. It is not a CPU submission interceptor.
#[derive(Clone)]
enum Event {
    Submit(Vec<Op>),
    Signal(Fence, u64),
    Wait(Fence, u64),
}
#[derive(Default)]
struct Scheduler {
    graph: Graph,
    pending: BTreeMap<Queue, VecDeque<Event>>,
    reads: Vec<Version>,
}
impl Scheduler {
    fn add_queue(&mut self, queue: Queue) {
        self.graph.add_queue(queue);
        self.pending.entry(queue).or_default();
    }
    fn enqueue(&mut self, queue: Queue, event: Event) -> Result<(), Error> {
        self.pending
            .get_mut(&queue)
            .ok_or(Error::UnknownQueue)?
            .push_back(event);
        Ok(())
    }
    // Returns the number processed; a zero return with pending events means
    // an unresolved future signal, not a claim of deadlock.
    fn drain(&mut self) -> Result<usize, Error> {
        let mut count = 0;
        loop {
            let mut progress = false;
            let queues: Vec<_> = self.pending.keys().copied().collect();
            for queue in queues {
                let Some(event) = self
                    .pending
                    .get(&queue)
                    .and_then(|events| events.front())
                    .cloned()
                else {
                    continue;
                };
                let mut staged = self.graph.clone();
                let result = match event {
                    Event::Submit(ref ops) => staged.submit(queue, ops),
                    Event::Signal(fence, value) => {
                        staged.signal(queue, fence, value).map(|()| Vec::new())
                    }
                    Event::Wait(fence, value) => {
                        staged.wait(queue, fence, value).map(|()| Vec::new())
                    }
                };
                match result {
                    Err(Error::UnknownFence) if matches!(event, Event::Wait(_, _)) => continue,
                    Err(error) => return Err(error),
                    Ok(reads) => {
                        self.graph = staged;
                        self.reads.extend(reads);
                        self.pending.get_mut(&queue).unwrap().pop_front();
                        count += 1;
                        progress = true;
                    }
                }
            }
            if !progress {
                return Ok(count);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph() -> Graph {
        let mut g = Graph::default();
        g.add_queue(1);
        g.add_queue(2);
        g.register(0x1000, 256).unwrap();
        g.register(0x2000, 256).unwrap();
        g
    }

    #[test]
    fn reversed_recording_order_uses_submission_order() {
        let mut g = graph();
        let first_recorded = [Op::Deserialize(0x1000, 10)];
        let second_recorded = [Op::Deserialize(0x1000, 20)];
        g.submit(1, &second_recorded).unwrap();
        g.submit(1, &first_recorded).unwrap();
        assert_eq!(g.submit(1, &[Op::Read(0x1000)]).unwrap(), [10]);
    }

    #[test]
    fn cross_queue_requires_ordered_signal_and_wait() {
        let mut g = graph();
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        assert_eq!(
            g.submit(2, &[Op::Read(0x1000)]),
            Err(Error::UnorderedAccess)
        );
        g.signal(1, 7, 1).unwrap();
        g.wait(2, 7, 1).unwrap();
        assert_eq!(g.submit(2, &[Op::Read(0x1000)]).unwrap(), [10]);
        assert_eq!(
            g.submit(1, &[Op::Deserialize(0x1000, 20)]),
            Err(Error::UnorderedAccess)
        );
        g.signal(2, 8, 1).unwrap();
        g.wait(1, 8, 1).unwrap();
        g.submit(1, &[Op::Deserialize(0x1000, 20)]).unwrap();
    }

    #[test]
    fn replay_publishes_new_generation_and_keeps_other_addresses() {
        let mut g = graph();
        // One recorded list refers to a mutable source token. The harness
        // supplies the source generation known at each queue execution.
        let list = [Op::DeserializeFrom(0x1000, 1), Op::Read(0x1000)];
        g.submit(1, &[Op::Deserialize(0x2000, 30)]).unwrap();
        g.sources.insert(1, 10);
        assert_eq!(g.submit(1, &list).unwrap(), [10]);
        // Caller must additionally prove completion before resubmitting a
        // direct list. This ordering-only model does not own GPU fences.
        g.sources.insert(1, 11);
        assert_eq!(g.submit(1, &list).unwrap(), [11]);
        assert_eq!(g.submit(1, &[Op::Read(0x2000)]).unwrap(), [30]);
    }

    #[test]
    fn rejection_is_transactional_and_unsupported_is_explicit() {
        let mut g = graph();
        assert_eq!(
            g.submit(1, &[Op::Deserialize(0x1000, 10), Op::Unsupported]),
            Err(Error::Unsupported)
        );
        assert_eq!(g.submit(1, &[Op::Read(0x1000)]), Err(Error::UnknownAddress));
        assert_eq!(g.wait(2, 7, 1), Err(Error::UnknownFence));
        assert_eq!(
            g.submit(99, &[Op::Deserialize(0x1000, 10)]),
            Err(Error::UnknownQueue)
        );
    }
    #[test]
    fn unordered_writes_and_future_fence_are_rejected() {
        let mut g = graph();
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        assert_eq!(
            g.submit(2, &[Op::Deserialize(0x1000, 20)]),
            Err(Error::UnorderedAccess)
        );
        assert_eq!(g.wait(2, 7, 2), Err(Error::UnknownFence));
        g.signal(1, 7, 1).unwrap();
        assert_eq!(g.signal(1, 7, 1), Err(Error::FenceOrder));
        assert_eq!(g.wait(2, 7, 2), Err(Error::UnknownFence));
        g.wait(2, 7, 1).unwrap();
        g.submit(2, &[Op::Deserialize(0x1000, 20)]).unwrap();
        assert_eq!(
            g.submit(1, &[Op::Read(0x1000)]),
            Err(Error::UnorderedAccess)
        );
        g.signal(2, 7, 2).unwrap();
        g.wait(1, 7, 2).unwrap();
        assert_eq!(g.submit(1, &[Op::Read(0x1000)]).unwrap(), [20]);
    }
    #[test]
    fn invalid_and_overlapping_ranges_are_rejected() {
        let mut g = graph();
        assert_eq!(g.register(0, 256), Err(Error::InvalidRange));
        assert_eq!(g.register(0x1201, 256), Err(Error::InvalidRange));
        assert_eq!(g.register(0x1200, 0), Err(Error::InvalidRange));
        assert_eq!(g.register(u64::MAX - 255, 512), Err(Error::InvalidRange));
        assert_eq!(g.register(0x1000, 256), Err(Error::Alias));
        assert_eq!(g.register(0x0f00, 512), Err(Error::Alias));
        g.register(0x1100, 256).unwrap();
        assert_eq!(
            g.submit(1, &[Op::Deserialize(0x1080, 2)]),
            Err(Error::UnknownAddress)
        );
    }
    #[test]
    fn replaced_resources_wait_for_every_queue_reader() {
        let mut g = graph();
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        g.signal(1, 7, 1).unwrap();
        g.wait(2, 7, 1).unwrap();
        g.submit(2, &[Op::Read(0x1000)]).unwrap();
        g.signal(2, 8, 1).unwrap();
        g.wait(1, 8, 1).unwrap();
        // The same source content is intentionally republished; resources
        // still have distinct ownership identities.
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        assert!(g.collect().is_empty());
        assert_eq!(g.complete(8, 2), Err(Error::UnknownFence));
        g.complete(7, 1).unwrap();
        assert!(g.collect().is_empty());
        g.complete(8, 1).unwrap();
        assert_eq!(g.collect(), [1]);
        assert!(g.resources.contains_key(&2));
        assert!(g.collect().is_empty());
    }

    #[test]
    fn failed_batch_does_not_publish_or_retire_resources() {
        let mut g = graph();
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        assert_eq!(
            g.submit(1, &[Op::Deserialize(0x1000, 11), Op::Unsupported]),
            Err(Error::Unsupported)
        );
        assert_eq!(g.resources.len(), 1);
        assert!(g.resources[&1].active);
        assert_eq!(g.submit(1, &[Op::Read(0x1000)]).unwrap(), [10]);
    }
    #[test]
    fn future_signal_wait_defers_consumer_and_its_reads() {
        let mut s = Scheduler::default();
        s.add_queue(1);
        s.add_queue(2);
        s.graph.register(0x1000, 256).unwrap();
        // Queue 2 is submitted first, exactly like the GPU retention control.
        s.enqueue(2, Event::Wait(7, 1)).unwrap();
        s.enqueue(2, Event::Submit(vec![Op::Read(0x1000)])).unwrap();
        assert_eq!(s.drain().unwrap(), 0);
        assert!(s.reads.is_empty());
        s.enqueue(1, Event::Submit(vec![Op::Deserialize(0x1000, 42)]))
            .unwrap();
        s.enqueue(1, Event::Signal(7, 1)).unwrap();
        assert_eq!(s.drain().unwrap(), 4);
        assert_eq!(s.reads, [42]);
        assert!(s.pending.values().all(VecDeque::is_empty));
    }

    #[test]
    fn unresolved_wait_does_not_publish_later_operations() {
        let mut s = Scheduler::default();
        s.add_queue(1);
        s.graph.register(0x1000, 256).unwrap();
        s.enqueue(1, Event::Wait(7, 1)).unwrap();
        s.enqueue(1, Event::Signal(7, 1)).unwrap();
        s.enqueue(1, Event::Submit(vec![Op::Deserialize(0x1000, 42)]))
            .unwrap();
        assert_eq!(s.drain().unwrap(), 0);
        assert!(s.graph.addresses.is_empty());
        assert_eq!(s.pending[&1].len(), 3);
    }
    #[test]
    fn independent_queue_readers_both_gate_retirement() {
        let mut g = graph();
        g.add_queue(3);
        g.submit(1, &[Op::Deserialize(0x1000, 10)]).unwrap();
        g.signal(1, 7, 1).unwrap();
        for queue in [2, 3] {
            g.wait(queue, 7, 1).unwrap();
            assert_eq!(g.submit(queue, &[Op::Read(0x1000)]).unwrap(), [10]);
            g.signal(queue, queue + 10, 1).unwrap();
        }
        g.wait(1, 12, 1).unwrap();
        g.wait(1, 13, 1).unwrap();
        g.submit(1, &[Op::Deserialize(0x1000, 11)]).unwrap();
        g.complete(12, 1).unwrap();
        assert!(g.collect().is_empty());
        g.complete(13, 1).unwrap();
        assert_eq!(g.collect(), [1]);
    }
}
