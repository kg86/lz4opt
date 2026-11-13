use super::*;
use crate::lz4::{encode, Options, BLOCK_64_KIB};

#[test]
fn snakree_encode_inspect_restore() {
    let cases: [(&str, Vec<u8>); 3] = [
        ("empty", Vec::new()),
        ("uncompressed", b"short unique bytes".to_vec()),
        ("compressed multiblock", b"abcabcabcXYZ".repeat(6000)),
    ];
    for (name, data) in cases {
        let mut encoded = Vec::new();
        let options = Options { block_size: BLOCK_64_KIB, ..Options::default() };
        encode(&mut encoded, data.as_slice(), &options).unwrap_or_else(|err| panic!("{name}: encode: {err}"));

        let root = parse(encoded.as_slice()).unwrap_or_else(|err| panic!("{name}: parse: {err}"));
        let doc = serde_json::to_vec(&root).unwrap_or_else(|err| panic!("{name}: serialize: {err}"));

        let mut restored = Vec::new();
        restore(&mut restored, doc.as_slice()).unwrap_or_else(|err| panic!("{name}: restore: {err}"));
        assert!(restored == encoded, "{name}: restored frame differs from encoded frame");
    }
}
