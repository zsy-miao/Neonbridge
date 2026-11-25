use super::{desktop_cbv_bytes, earlier_page};

#[test]
fn an_earlier_page_puts_a_view_on_a_whole_element() {
    // ProjectScarlett: 20-byte elements starting 352 bytes into a page.
    let page = earlier_page(0xaa8f_0160, 20, 0xaa00_0000).unwrap();
    assert_eq!(page, 0xaa8c_0000);
    assert_eq!((0xaa8f_0160 - page) % 20, 0);
    // The allocation starts at the view's own page: nothing earlier to use.
    assert_eq!(earlier_page(0xaa8f_0160, 20, 0xaa8f_0000), None);
    // No page boundary can fix an offset the stride's page factor rules out.
    assert_eq!(earlier_page(0xaa8f_0162, 20, 0), None);
    assert_eq!(earlier_page(0x2_0028, 20, 0), Some(0x2_0000));
}

#[test]
fn desktop_cbv_size_is_a_256_byte_multiple_up_to_the_limit() {
    assert_eq!(desktop_cbv_bytes(0), 0);
    assert_eq!(desktop_cbv_bytes(1), 256);
    assert_eq!(desktop_cbv_bytes(144), 256);
    assert_eq!(desktop_cbv_bytes(256), 256);
    assert_eq!(desktop_cbv_bytes(257), 512);
    assert_eq!(desktop_cbv_bytes(65536), 65536);
    assert_eq!(desktop_cbv_bytes(65537), 65537);
}
