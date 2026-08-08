use prost::Message;

mod v1 {
    include!(env!("SCHEMAHUB_PROTO_V1_RS"));
}

mod v2 {
    include!(env!("SCHEMAHUB_PROTO_V2_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: protobuf-compat <new-record-output>")?;
    let old_order = v1::OrderRecord {
        id: "order-1001".to_string(),
        total_cents: 12_500,
        created_at_unix_ms: 1_784_764_800_000,
    };

    // Act
    let old_bytes = old_order.encode_to_vec();
    let old_read_by_new = v2::OrderRecord::decode(old_bytes.as_slice())?;
    let new_order = v2::OrderRecord {
        id: "order-1002".to_string(),
        total_cents: 8_750,
        created_at_unix_ms: 1_784_764_801_000,
        settlement_currency: "USD".to_string(),
    };
    let new_bytes = new_order.encode_to_vec();
    let new_read_by_old = v1::OrderRecord::decode(new_bytes.as_slice())?;
    std::fs::write(output, &new_bytes)?;

    // Assert
    assert_eq!(old_read_by_new.id, old_order.id);
    assert_eq!(old_read_by_new.total_cents, old_order.total_cents);
    assert_eq!(old_read_by_new.settlement_currency, "");
    assert_eq!(new_read_by_old.id, new_order.id);
    assert_eq!(new_read_by_old.total_cents, new_order.total_cents);
    println!(
        "protobuf old->new and new->old decoding passed ({} and {} bytes)",
        old_bytes.len(),
        new_bytes.len()
    );
    Ok(())
}
