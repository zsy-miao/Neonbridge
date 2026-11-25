import unittest
from tools.check_d3d12_regressions import validate,DIGEST

class D3D12RegressionTests(unittest.TestCase):
    def test_adapter_and_exit_are_checked(self):
        line="d3d12-indirect PASS pci=10de:2704 list=59 words=64/64"
        self.assertTrue(validate("indirect","10de:2704",0,line)[0])
        self.assertFalse(validate("indirect","1002:1234",0,line)[0])
        self.assertFalse(validate("indirect","10de:2704",1,line)[0])
        self.assertFalse(validate("indirect","10de:2704",0,line+"\n"+line)[0])
        self.assertFalse(validate("indirect-scarlett","10de:2704",0,line)[0])
    def test_null_checkpoint_must_match(self):
        line="d3d12-null-usage PASS pci=10de:2704 words=64/64 recorded=64 valid=2"
        logs=[line]+[f"TuxBox d3d12_xs: substituted null compute pipeline for an untranslated shader ({n} so far)" for n in (1,2)]
        for interval,count in ((1,1),(2,63)):
            logs.append(f"TuxBox d3d12_xs: null usage interval recorded={count} display_bound=0 tracked=1 overflow=0 overflow_display_bound=0 compute={count} raster=0 interval={interval} trigger=recording_checkpoint")
            logs.append(f"TuxBox d3d12_xs: null usage rank kind=compute CS={DIGEST} recorded={count} display_bound=0 interval={interval} trigger=recording_checkpoint")
        output="\n".join(logs)
        self.assertTrue(validate("null-usage","10de:2704",0,output)[0])
        self.assertFalse(validate("null-usage","10de:2704",0,output.replace("compute=63","compute=64"))[0])
        self.assertFalse(validate("null-usage","10de:2704",0,line)[0])

    def test_rejects_fallback_and_contradictory_receipts(self):
        line="d3d12-indirect PASS pci=10de:2704 list=59 words=64/64"
        bad=["TuxBox d3d12_xs: substituted null compute pipeline for an untranslated shader (1 so far)",
             "TuxBox d3d12_xs: null usage rank kind=compute CS="+DIGEST,
             line.replace("10de:2704","1002:1234"),
             "d3d12-draw PASS pci=10de:2704 red=36 blue=220"]
        for other in bad:
            self.assertFalse(validate("indirect","10de:2704",0,line+"\n"+other)[0])
        self.assertFalse(validate("indirect","10de:2704",0,line+" words=1/64")[0])
        negative="d3d12-null-disabled PASS pci=10de:2704 rejected=1 valid=1"
        self.assertTrue(validate("null-disabled","10de:2704",0,negative)[0])
        self.assertFalse(validate("null-disabled","10de:2704",0,negative+"\n"+bad[0])[0])
    def test_buffer_import_requires_exact_backing_counts(self):
        line="d3d12-buffer-import PASS pci=10de:2704 pages=2 words=1536/1536 passes=2 aliases=2"
        records=[("pool_open",i) for i in range(1,4)]+[("pool_hit",i) for i in range(1,3)]+[("imported",i) for i in range(1,6)]
        logs=[line]+[f"TuxBox d3d12_xs: buffer backing path={p} count={i} source=place_virtual bytes=65536" for p,i in records]
        self.assertTrue(validate("buffer-import","10de:2704",0,"\n".join(logs))[0])
        self.assertFalse(validate("buffer-import","10de:2704",0,"\n".join(logs[:-1]))[0])
        self.assertFalse(validate("buffer-import","10de:2704",0,"\n".join(logs+[
            "TuxBox d3d12_xs: buffer backing path=device_standin count=1 source=place_virtual bytes=65536"]))[0])

    def test_late_commit_checks_both_imports_and_no_standin(self):
        line="d3d12-buffer-layout PASS pci=10de:2704 slot=133 words=512/512 VA-offsets=0,4"
        logs=[line]+[f"TuxBox d3d12_xs: buffer backing path={p} count={i} source=place_virtual_layout bytes={1024+4*(i-1)}"
                     for p in ("imported","commit_retry") for i in (1,2)]
        self.assertTrue(validate("buffer-layout","10de:2704",0,"\n".join(logs))[0])
        reuse=[f"TuxBox d3d12_xs: buffer backing path={p} count=1 source=place_virtual_layout bytes=65536"
               for p in ("page_open","page_hit","span_open","span_hit")]
        self.assertTrue(validate("buffer-layout","10de:2704",0,"\n".join(logs+reuse))[0])
        standin="TuxBox d3d12_xs: buffer backing path=device_standin count=1 source=place_virtual_layout bytes=1024"
        self.assertFalse(validate("buffer-layout","10de:2704",0,"\n".join(logs+reuse+[standin]))[0])
        self.assertFalse(validate("buffer-layout","10de:2704",0,"\n".join(logs[:-1]))[0])
        self.assertFalse(validate("buffer-layout","10de:2704",0,"\n".join(logs).replace("commit_retry","device_standin"))[0])
        negative="d3d12-buffer-import-reject PASS pci=10de:2704 rejected=1"
        record="TuxBox d3d12_xs: buffer backing path=strict_rejected count=1 source=place_virtual bytes=65536"
        self.assertTrue(validate("buffer-import-reject","10de:2704",0,negative+"\n"+record)[0])
        self.assertFalse(validate("buffer-import-reject","10de:2704",0,negative)[0])

    def test_remap_requires_retirement_and_reimport(self):
        line="d3d12-buffer-remap PASS pci=10de:2704 words=768/768 identical=1 replacement=1"
        records=[("pool_open",1),("pool_open",2),("pool_hit",1)]+[("imported",i) for i in range(1,4)]
        logs=[line]+[f"TuxBox d3d12_xs: buffer backing path={p} count={i} source=place_virtual_layout bytes=65536" for p,i in records]
        logs.append("TuxBox d3d12_xs: page remap replaced imported range retired stale GPU import")
        output="\n".join(logs)
        self.assertTrue(validate("buffer-remap","10de:2704",0,output)[0])
        self.assertFalse(validate("buffer-remap","10de:2704",0,"\n".join(logs[:-1]))[0])
        self.assertFalse(validate("buffer-remap","10de:2704",0,output.replace("pool_open count=2","pool_hit count=2"))[0])
        self.assertFalse(validate("buffer-remap","10de:2704",0,output+"\n"+logs[-1])[0])

    def test_deferred_alias_refresh_requires_both_overlaps(self):
        receipt="d3d12-alias-deferred PASS pci=10de:2704 overlaps=2 old=verified refreshed=verified no_cpu_wait=1"
        line="placed texture refresh deferred until an overlapping write is published"
        output="\n".join([receipt,line,line,"alias deferred observed ms=0"])
        self.assertTrue(validate("alias-deferred","10de:2704",0,output)[0])
        self.assertFalse(validate("alias-deferred","10de:2704",0,output.replace("ms=0","ms=1999"))[0])
        self.assertFalse(validate("alias-deferred","10de:2704",0,output.replace(line,"unrelated",1))[0])
        self.assertFalse(validate("alias-deferred","10de:2704",0,output+"\nFailed to signal event")[0])

    def test_root_cbv_window_requires_full_imports(self):
        line="d3d12-root-cbv-window PASS pci=10de:2704 words=120/120 window=65536 offsets=0,256,65280,65536,65792"
        records=[f"TuxBox d3d12_xs: buffer backing path=imported count={i} source=view bytes={n}" for i,n in enumerate((65536,131072,131072,65536,131072),1)]
        output="\n".join([line]+records)
        self.assertTrue(validate("root-cbv-window","10de:2704",0,output)[0])
        self.assertFalse(validate("root-cbv-window","10de:2704",0,output.replace("131072","65536"))[0])
        self.assertFalse(validate("root-cbv-window","10de:2704",0,output+"\nbound with a short range")[0])
        self.assertFalse(validate("root-cbv-window","10de:2704",0,output+"\nsubstituted null compute pipeline")[0])
