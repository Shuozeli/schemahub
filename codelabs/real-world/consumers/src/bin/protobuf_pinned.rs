use prost::Message;

mod schema {
    include!(env!("SCHEMAHUB_PINNED_PROTO_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: protobuf-pinned <record-output>")?;
    let order = schema::OrderRecord {
        id: "order-pinned-1001".to_string(),
        created_at_unix_ms: 1_784_764_800_000,
        payload: b"persisted-business-payload".to_vec(),
    };

    // Act
    let bytes = order.encode_to_vec();
    std::fs::write(&output, &bytes)?;
    let restored = schema::OrderRecord::decode(std::fs::read(&output)?.as_slice())?;

    // Assert
    assert_eq!(restored, order);
    assert_eq!(restored.payload, b"persisted-business-payload");
    println!(
        "pinned generated binding encoded and decoded {} bytes",
        bytes.len()
    );
    Ok(())
}
