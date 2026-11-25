use super::*;

#[test]
fn only_failures_and_stand_ins_make_rows() {
    let stages = [("MS", Some("aa".to_string())), ("PS", None)];
    assert_eq!(row("slot86", 0, false, &stages, &[]), None);
    assert_eq!(
        row("slot86", 0x8000_4001_u32 as i32, false, &stages, &["MS.missing=bb".into()]).unwrap(),
        "0x80004001\tslot86\tMS=aa PS=unknown MS.missing=bb"
    );
    assert_eq!(row("slot62", 0, true, &[], &[]).unwrap(), "stand-in\tslot62\t-");
}

#[test]
fn repeats_are_counted_and_sorted_by_frequency() {
    let mut record = Record::default();
    record.add("b".into());
    record.add("a".into());
    record.add("a".into());
    assert_eq!(record.render(), "count\toutcome\tpath\tstages\n2\ta\n1\tb\n");
}

#[test]
fn rows_past_the_limit_are_still_counted() {
    let mut record = Record::default();
    for index in 0..LIMIT + 3 {
        record.add(index.to_string());
    }
    record.add("0".into());
    assert_eq!(record.rows.len(), LIMIT);
    assert_eq!(record.dropped, 3);
    assert!(record.render().ends_with("3\tnot listed\t-\t-\n"));
}

#[test]
fn a_failed_pipeline_reaches_the_file() {
    let code = *b"shader";
    begin();
    stage("CS", code.as_ptr(), code.len());
    lookup("CS", "key", true);
    lookup("CS", "key", true);
    // SAFETY: `code` is alive across the call.
    let first = unsafe { finish("slot62", -1, |bytes| Some(format!("len{}", bytes.len()))) };
    assert!(first.is_some());
    let file = std::env::temp_dir().join(format!("tuxbox-failures-{}.tsv", std::process::id()));
    write_if_changed(&RECORD, &file);
    let text = std::fs::read_to_string(&file).unwrap();
    let _ = std::fs::remove_file(&file);
    assert!(text.contains("1\t0xffffffff\tslot62\tCS=len6 CS.used=key\n"), "{text}");
}

#[test]
fn wave_refusal_records_stage_reason() {
    let code = *b"shader";
    begin();
    stage("CS", code.as_ptr(), code.len());
    lookup("CS", "key", true);
    refusal("CS", "host-lacks-wave64");
    stand_in();
    // SAFETY: `code` stays live across `finish`.
    let _ = unsafe { finish("slot62", 0, |bytes| Some(format!("len{}", bytes.len()))) };
    let rendered = RECORD.lock().unwrap_or_else(|e| e.into_inner()).render();
    assert!(
        rendered.contains("stand-in\tslot62\tCS=len6 CS.used=key CS.refused=host-lacks-wave64\n"),
        "{rendered}"
    );
}
