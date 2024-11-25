fn main() {
    femtopb_build::compile_protos(
        &[
            "protobufs/punches.proto",
            "protobufs/status.proto",
            "protobufs/timestamp.proto",
        ],
        &["protobufs"],
    )
    .unwrap();
}
