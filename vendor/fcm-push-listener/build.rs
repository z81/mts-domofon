extern crate prost_build;

fn main() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc is available");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config
        .compile_protos(
            &["src/proto/checkin.proto", "src/proto/mcs.proto"],
            &["src/proto"],
        )
        .expect("FCM protobuf definitions compile");
}
