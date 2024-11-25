fn main() {
    femtopb_build::compile_protos(
        &[
            "../python/src/protobufs/punches.proto",
            "../python/src/protobufs/status.proto",
            "../python/src/protobufs/timestamp.proto",
        ],
        &["../python/src/protobufs"],
    )
    .unwrap();
}
