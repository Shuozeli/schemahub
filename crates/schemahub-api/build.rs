fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = "proto";
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &[
                "proto/schemahub/v1/enums.proto",
                "proto/schemahub/v1/refs.proto",
                "proto/schemahub/v1/resources.proto",
                "proto/schemahub/v1/errors.proto",
                "proto/schemahub/v1/mutations.proto",
                "proto/schemahub/v1/schema_service.proto",
                "proto/schemahub/v1/ref_service.proto",
                "proto/schemahub/v1/exploration_service.proto",
                "proto/schemahub/v1/codegen_service.proto",
                "proto/schemahub/v1/project_service.proto",
                "proto/schemahub/v1/admin_service.proto",
            ],
            &[proto_dir],
        )?;
    Ok(())
}
