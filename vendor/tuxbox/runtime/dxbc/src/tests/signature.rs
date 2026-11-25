use super::*;
use crate::container::tests::container_with;

fn parameter_words(words: &[u32], names: &[u8]) -> Vec<u8> {
    let mut bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
    bytes.extend_from_slice(names);
    bytes
}

#[test]
fn reads_each_format_with_relocated_parameter_table() {
    for (tag, stream, precision) in [
        (b"ISGN", false, false),
        (b"OSGN", false, false),
        (b"OSG5", true, false),
        (b"ISG1", true, true),
        (b"OSG1", true, true),
    ] {
        let name_offset = 12 + 24 + 4 * u32::from(stream) + 4 * u32::from(precision);
        let mut words = vec![1, 12, 0xff00_0088];
        if stream {
            words.push(2);
        }
        words.extend([name_offset, 5, 7, 1, 13, 0xf1e2_030b]);
        if precision {
            words.push(4);
        }
        let body = parameter_words(&words, b"Sample\0");
        let file = container_with(&[(tag, &body)]);
        assert_eq!(
            signature(&file, &[tag]).unwrap(),
            [SigElement {
                name: "Sample".into(),
                index: 5,
                sysval: 7,
                comp_type: 1,
                register: 13,
                mask: 0x0b,
                rw_mask: 3,
            }]
        );
    }
}

#[test]
fn reads_two_rows_referencing_a_shared_semantic_name() {
    let body = parameter_words(
        &[
            2, 8, 0, 72, 0, 0, 3, 12, 0x0f0f, 0, 0, 72, 1, 0, 3, 13, 0x0303, 0,
        ],
        b"TexCoord\0",
    );
    let params = parse_signature(&body).unwrap();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, params[1].name);
    assert_eq!((params[1].index, params[1].register), (1, 13));
    assert!(
        parse_signature(&parameter_words(&[0, 8], &[]))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn counts_each_owned_copy_of_a_shared_name_toward_the_limit() {
    let row = [56, 0, 0, 3, 0, 0x0f0f];
    let mut words = vec![2, 8];
    words.extend(row);
    words.extend(row);
    let body = parameter_words(&words, b"UV\0");
    let bytes = 2 * (size_of::<SigElement>() + 2);
    let params = read_parameters(&body, Layout::Legacy, bytes).unwrap();
    assert_eq!(
        (params[0].name.as_str(), params[1].name.as_str()),
        ("UV", "UV")
    );
    assert!(read_parameters(&body, Layout::Legacy, bytes - 1).is_err());
}

#[test]
fn checks_table_extent_and_semantic_string_extent() {
    for (count, start, name_offset, names) in [
        (u32::MAX, 8, 40, b"OK\0".as_slice()),
        (1, 0, 40, b"OK\0"),
        (1, 12, 40, b"OK\0"),
        (1, u32::MAX, 40, b"OK\0"),
        (1, 8, 8, b"OK\0"),
        (1, 8, 39, b"OK\0"),
        (1, 8, u32::MAX, b"OK\0"),
        (1, 8, 40, b"\0"),
        (1, 8, 40, b"ABC"),
    ] {
        let body =
            parameter_words(&[count, start, 0, name_offset, 0, 0, 1, 1, 0x101, 0], names);
        assert!(
            parse_signature(&body).is_err(),
            "count {count} offset {start} name {name_offset}"
        );
    }
}
