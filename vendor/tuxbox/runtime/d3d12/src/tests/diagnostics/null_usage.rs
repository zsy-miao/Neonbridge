use super::*;
fn tag(kind: &'static str, stages: &str) -> Arc<Tag> {
    Arc::new(Tag { kind, stages: stages.into() })
}
#[test]
fn counts_by_shader_not_allocation_identity_and_prioritizes_display() {
    let mut usage = Usage::new(3);
    let compute = tag("compute", "CS=a");
    for _ in 0..4 { usage.record(&compute, false); }
    usage.record(&tag("raster", "VS=b PS=c"), true);
    usage.record(&tag("raster", "VS=b PS=c"), false);
    let result = usage.drain();
    assert_eq!(result[0], "null usage interval recorded=6 display_bound=1 tracked=2 overflow=0 overflow_display_bound=0 compute=4 raster=2 interval=1");
    assert!(result[1].contains("VS=b PS=c recorded=2 display_bound=1"));
    assert!(result[2].contains("CS=a recorded=4 display_bound=0"));
    assert!(usage.drain()[0].contains("recorded=0"));
}
#[test]
fn recording_checkpoints_are_independent_of_present_samples() {
    let mut usage = Usage::new(2);
    let tag = tag("compute", "CS=a");
    let mut checkpoints = Vec::new();
    for n in 1..=150 {
        if usage.record(&tag, false) { checkpoints.push(n); }
        if n == 10 || n == 100 { usage.drain(); }
    }
    assert_eq!(checkpoints, [1, 64, 128]);
}
#[test]
fn caps_distinct_keys_without_losing_aggregate_counts() {
    let mut usage = Usage::new(1);
    let a = tag("raster", "VS=a");
    let b = tag("raster", "VS=b");
    usage.record(&a, false);
    usage.record(&b, true);
    usage.record(&a, true);
    usage.record(&b, false);
    let result = usage.drain();
    assert!(result[0].contains("recorded=4 display_bound=2 tracked=1 overflow=2 overflow_display_bound=1"));
    assert!(result[1].contains("VS=a recorded=2 display_bound=1"));
    assert_eq!(Arc::strong_count(&a), 1);
    assert_eq!(Arc::strong_count(&b), 1);
}
