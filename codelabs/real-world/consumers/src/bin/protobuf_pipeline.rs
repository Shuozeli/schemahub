use prost::Message;

mod schema {
    include!(env!("SCHEMAHUB_PIPELINE_PROTO_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("expected produce or consume")?;
    let path = args.next().ok_or("expected data path")?;

    // Act
    match mode.as_str() {
        "produce" => {
            let order = schema::PipelineOrder {
                id: "pipeline-order-1".to_string(),
                quantity: 7,
            };
            std::fs::write(&path, order.encode_to_vec())?;
        }
        "consume" => {
            let bytes = std::fs::read(&path)?;
            let order = schema::PipelineOrder::decode(bytes.as_slice())?;

            // Assert
            assert_eq!(order.id, "pipeline-order-1");
            assert_eq!(order.quantity, 7);
        }
        _ => return Err(format!("unknown mode {mode:?}").into()),
    }

    println!("protobuf pipeline {mode} passed for {path}");
    Ok(())
}
