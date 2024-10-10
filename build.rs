use std::{env, path::PathBuf};
use walkdir::WalkDir;

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let descriptor_file = out.join("descriptors.bin");

    let protobufs_dir = "src/protobufs/";
    println!("cargo:rerun-if-changed={}", protobufs_dir);

    let protos: Vec<_> = WalkDir::new(protobufs_dir)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext.to_str().unwrap() == "proto")
        })
        .map(|entry| entry.path().to_owned())
        .collect();

    // Allows protobuf compilation without installing the `protoc` binary
    if std::env::var("PROTOC").ok().is_some() {
        println!("Using PROTOC set in environment.");
    } else {
        match protoc_bin_vendored::protoc_bin_path() {
            Ok(protoc_path) => {
                println!("Setting PROTOC to protoc-bin-vendored version.");
                std::env::set_var("PROTOC", protoc_path);
            }
            Err(err) => {
                println!("Install protoc yourself, protoc-bin-vendored failed: {err}");
            }
        }
    }

    let mut config = prost_build::Config::new();
    config
        .file_descriptor_set_path(&descriptor_file)
        .compile_protos(&protos, &[protobufs_dir])?;

    Ok(())
}
