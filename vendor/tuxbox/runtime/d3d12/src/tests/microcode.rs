use super::*;

#[test]
fn delivers_the_console_record_and_normalizes_driver_status() {
    let payloads: [&[u8]; 3] = [&[1; 3], &[2; 5], &[3; 7]];
    let images = payloads.map(|bytes| (bytes.len(), bytes.as_ptr()));
    for (status, expected) in [
        (STATUS_SUCCESS, S_OK),
        (NTSTATUS(1), E_FAIL),
        (NTSTATUS(0x40000001), E_FAIL),
        (STATUS_UNSUCCESSFUL, E_FAIL),
    ] {
        let mut called = false;
        let actual = submit(0x12345678, images, |request| {
            called = true;
            assert_eq!(request.selector, 0x12345678);
            assert_eq!(request.reserved, 0);
            for (image, payload) in request.images.iter().zip(payloads) {
                assert_eq!(image.address, payload.as_ptr());
                assert_eq!(image.bytes, payload.len());
            }
            status
        });
        assert!(called);
        assert_eq!(actual, expected);
    }
    assert_eq!(
        submit(0, images, |_| panic!("invalid selector reached driver")),
        E_INVALIDARG
    );
}
