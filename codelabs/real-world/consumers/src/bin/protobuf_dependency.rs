use prost::Message;

mod v1 {
    include!(env!("SCHEMAHUB_DEPENDENCY_PROTO_V1_RS"));
}

mod v2 {
    include!(env!("SCHEMAHUB_DEPENDENCY_PROTO_V2_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: protobuf-dependency <new-payment-output>")?;
    let old_payment = v1::PaymentCaptured {
        payment_id: "payment-1001".to_string(),
        amount: Some(v1::payments::money::v1::Money {
            units: 125,
            nanos: 500_000_000,
        }),
    };

    // Act
    let old_bytes = old_payment.encode_to_vec();
    let old_read_by_new = v2::PaymentCaptured::decode(old_bytes.as_slice())?;
    let new_payment = v2::PaymentCaptured {
        payment_id: "payment-1002".to_string(),
        amount: Some(v2::payments::money::v1::Money {
            units: 87,
            nanos: 0,
            currency_code: "USD".to_string(),
        }),
    };
    let new_bytes = new_payment.encode_to_vec();
    let new_read_by_old = v1::PaymentCaptured::decode(new_bytes.as_slice())?;
    std::fs::write(output, &new_bytes)?;

    // Assert
    assert_eq!(
        old_read_by_new
            .amount
            .as_ref()
            .map(|amount| amount.currency_code.as_str()),
        Some("")
    );
    assert_eq!(new_read_by_old.payment_id, new_payment.payment_id);
    assert_eq!(
        new_read_by_old.amount.as_ref().map(|amount| amount.units),
        Some(87)
    );
    println!(
        "dependency closure decoded old/new payments ({} and {} bytes)",
        old_bytes.len(),
        new_bytes.len()
    );
    Ok(())
}
