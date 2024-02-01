use prost_wkt_build::*;
use std::{env, path::PathBuf};
use walkdir::WalkDir;

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let descriptor_file = out.join("descriptors.bin");

    let protobufs_dir = "src/protobufs/";
    println!("cargo:rerun-if-changed={}", protobufs_dir);

    // Allows protobuf compilation without installing the `protoc` binary
    let protoc_path = protoc_bin_vendored::protoc_bin_path().unwrap();
    std::env::set_var("PROTOC", protoc_path);

    let mut protos = vec![];
    for entry in WalkDir::new(protobufs_dir)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext.to_str().unwrap() == "proto")
        })
    {
        let path = entry.path();
        protos.push(path.to_owned());
    }

    let mut config = prost_build::Config::new();
    config
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .extern_path(".google.protobuf.Timestamp", "::prost_wkt_types::Timestamp")
        .file_descriptor_set_path(&descriptor_file)
        .compile_protos(&protos, &[protobufs_dir])?;

    let descriptor_bytes = std::fs::read(descriptor_file).unwrap();

    let descriptor = FileDescriptorSet::decode(&descriptor_bytes[..]).unwrap();
    prost_wkt_build::add_serde(out, descriptor);

    Ok(())
}
