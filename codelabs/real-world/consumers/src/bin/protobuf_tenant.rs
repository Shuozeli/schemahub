use prost::Message;

mod schema {
    include!(env!("SCHEMAHUB_TENANT_PROTO_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let entry = schema::LedgerEntry {
        entry_id: "ledger-1001".to_string(),
        amount_micros: 42_500_000,
    };

    // Act
    let bytes = entry.encode_to_vec();
    let restored = schema::LedgerEntry::decode(bytes.as_slice())?;

    // Assert
    assert_eq!(restored, entry);
    assert_eq!(restored.amount_micros, 42_500_000);
    println!(
        "authorized tenant binding encoded and decoded {} bytes",
        bytes.len()
    );
    Ok(())
}
