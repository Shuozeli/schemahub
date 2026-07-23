use prost::Message;

mod resolved {
    include!(env!("SCHEMAHUB_CONCURRENT_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let order = resolved::OrderRecord {
        id: "order-concurrent-1".to_string(),
        human_note: "gift wrap".to_string(),
        agent_note: "fraud score reviewed".to_string(),
    };

    // Act
    let bytes = order.encode_to_vec();
    let decoded = resolved::OrderRecord::decode(bytes.as_slice())?;

    // Assert
    assert_eq!(decoded, order);
    assert_eq!(decoded.human_note, "gift wrap");
    assert_eq!(decoded.agent_note, "fraud score reviewed");
    println!(
        "resolved concurrent binding encoded and decoded {} bytes",
        bytes.len()
    );
    Ok(())
}
