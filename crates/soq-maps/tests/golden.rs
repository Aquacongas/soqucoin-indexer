#[test]
fn golden_vectors() {
    let expected = include_str!("../../../tests/fixtures/maps-v1-golden.txt")
        .lines()
        .collect::<Vec<_>>();
    for (i, n) in [0u8, 1, 42, 127, 254, 255].into_iter().enumerate() {
        let p = soq_maps::parameters(hex::encode([n; 32]), n as u64).unwrap();
        let art = soq_maps::generate(p).unwrap();
        assert_eq!(art.content_sha256, expected[i]);
    }
}
