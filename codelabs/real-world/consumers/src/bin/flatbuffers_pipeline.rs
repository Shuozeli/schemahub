pub mod schema {
    include!(env!("SCHEMAHUB_PIPELINE_FBS_RS"));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Arrange
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("expected produce or consume")?;
    let path = args.next().ok_or("expected data path")?;

    // Act
    match mode.as_str() {
        "produce" => {
            let mut builder = flatbuffers::FlatBufferBuilder::new();
            let event_id = builder.create_string("pipeline-event-1");
            let order_id = builder.create_string("pipeline-order-1");
            let event = schema::pipeline::PipelineEvent::create(
                &mut builder,
                &schema::pipeline::PipelineEventArgs {
                    event_id: Some(event_id),
                    order_id: Some(order_id),
                },
            );
            builder.finish_minimal(event);
            std::fs::write(&path, builder.finished_data())?;
        }
        "consume" => {
            let bytes = std::fs::read(&path)?;
            let event = schema::root_as_pipeline_event(&bytes)?;

            // Assert
            assert_eq!(event.event_id(), Some("pipeline-event-1"));
            assert_eq!(event.order_id(), Some("pipeline-order-1"));
        }
        _ => return Err(format!("unknown mode {mode:?}").into()),
    }

    println!("flatbuffers pipeline {mode} passed for {path}");
    Ok(())
}
