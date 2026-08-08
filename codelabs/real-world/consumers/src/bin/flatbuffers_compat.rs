pub mod v1 {
    include!(env!("SCHEMAHUB_FBS_V1_RS"));
}

pub mod v2 {
    include!(env!("SCHEMAHUB_FBS_V2_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: flatbuffers-compat <new-record-output>")?;
    let mut old_builder = flatbuffers::FlatBufferBuilder::new();
    let old_id = old_builder.create_string("event-1001");
    let legacy_session = old_builder.create_string("legacy-session-7");
    let old_event = v1::telemetry::MobileEvent::create(
        &mut old_builder,
        &v1::telemetry::MobileEventArgs {
            event_id: Some(old_id),
            captured_at_unix_ms: 1_784_764_800_000,
            legacy_session_id: Some(legacy_session),
        },
    );
    old_builder.finish_minimal(old_event);

    // Act
    let old_bytes = old_builder.finished_data().to_vec();
    let old_read_by_new = v2::root_as_mobile_event(&old_bytes)?;

    let mut new_builder = flatbuffers::FlatBufferBuilder::new();
    let new_id = new_builder.create_string("event-1002");
    let new_event = v2::telemetry::MobileEvent::create(
        &mut new_builder,
        &v2::telemetry::MobileEventArgs {
            event_id: Some(new_id),
            captured_at_unix_ms: 1_784_764_801_000,
            sampling_rate: 0.25,
            ..Default::default()
        },
    );
    new_builder.finish_minimal(new_event);
    let new_bytes = new_builder.finished_data().to_vec();
    let new_read_by_old = v1::root_as_mobile_event(&new_bytes)?;
    std::fs::write(output, &new_bytes)?;

    // Assert
    assert_eq!(old_read_by_new.event_id(), Some("event-1001"));
    assert_eq!(old_read_by_new.sampling_rate(), 1.0);
    assert_eq!(new_read_by_old.event_id(), Some("event-1002"));
    assert_eq!(new_read_by_old.captured_at_unix_ms(), 1_784_764_801_000);
    println!(
        "flatbuffers old->new defaults and new->old decoding passed ({} and {} bytes)",
        old_bytes.len(),
        new_bytes.len()
    );
    Ok(())
}
