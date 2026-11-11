// Regression coverage for RUSTSEC-2024-0429's borrowed-string out-pointer.
// Use the real GLib FFI; keep each owning Variant alive after iterator drop.
use glib::{prelude::*, Variant};

fn strings(values: &[&str]) -> Variant {
    Variant::array_from_iter::<String>(values.iter().map(|value| value.to_string().to_variant()))
}

#[test]
fn variant_str_iter_backport_empty_is_fused() {
    let variant = strings(&[]);
    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.len(), 0);
    assert_eq!(iter.size_hint(), (0, Some(0)));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    assert_eq!(iter.nth(0), None);
    assert_eq!(iter.nth_back(0), None);
    assert_eq!(variant.array_iter_str().unwrap().last(), None);
}

#[test]
fn variant_str_iter_backport_single_borrow_survives_iterator() {
    let variant = strings(&["Gruesse \u{00df} \u{1f980}"]);
    let borrowed = {
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.len(), 1);
        let value = iter.next().unwrap();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.next_back(), None);
        value
    };
    assert_eq!(borrowed, "Gruesse \u{00df} \u{1f980}");
    assert_eq!(variant.array_iter_str().unwrap().nth(0), Some(borrowed));
    assert_eq!(variant.array_iter_str().unwrap().nth_back(0), Some(borrowed));
    assert_eq!(variant.array_iter_str().unwrap().last(), Some(borrowed));
}

#[test]
fn variant_str_iter_backport_unicode_mixed_directions() {
    let expected = [
        "Gr\u{00fc}\u{00df}e",
        "\u{6771}\u{4eac}",
        "\u{1f980}",
        "e\u{0301}",
        "",
        "\u{b05d}",
    ];
    let variant = strings(&expected);
    let (first, crab, last, combining) = {
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.len(), 6);
        let first = iter.next().unwrap();
        assert_eq!(iter.len(), 5);
        let crab = iter.nth(1).unwrap();
        assert_eq!(iter.len(), 3);
        let last = iter.next_back().unwrap();
        assert_eq!(iter.len(), 2);
        let combining = iter.nth_back(1).unwrap();
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        (first, crab, last, combining)
    };
    assert_eq!(
        (first, crab, last, combining),
        (expected[0], expected[2], expected[5], expected[3])
    );
    assert_eq!(variant.array_iter_str().unwrap().last(), Some(expected[5]));
    let mut from_front = variant.array_iter_str().unwrap();
    assert_eq!(from_front.nth(4), Some(""));
    assert_eq!(from_front.len(), 1);
    assert_eq!(from_front.last(), Some(expected[5]));
    let mut from_back = variant.array_iter_str().unwrap();
    assert_eq!(from_back.nth_back(4), Some(expected[1]));
    assert_eq!(from_back.len(), 1);
    assert_eq!(from_back.last(), Some(expected[0]));
}

#[test]
fn variant_str_iter_backport_out_of_range_exhausts() {
    let variant = strings(&["\u{03b1}", "\u{03b2}"]);
    let mut front = variant.array_iter_str().unwrap();
    assert_eq!(front.nth(usize::MAX), None);
    assert_eq!(front.len(), 0);
    assert_eq!(front.next_back(), None);
    let mut back = variant.array_iter_str().unwrap();
    assert_eq!(back.nth_back(usize::MAX), None);
    assert_eq!(back.len(), 0);
    assert_eq!(back.next(), None);
}
