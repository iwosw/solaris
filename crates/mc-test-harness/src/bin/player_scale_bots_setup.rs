const SETUP_COMMAND_BATCH: usize = 32;

impl Bot {
    async fn setup_fence(
        &mut self,
        client: &mut Client,
        scope: &str,
        preparing: bool,
    ) -> Result<()> {
        // Exact own echo is an input-ordered fence; no-op commands need not emit
        // feedback. This chat belongs to preparation, never measured chat offers.
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let message = format!(
            "scale_setup_fence_{}_{}_{}",
            now.as_nanos(),
            self.index,
            scope
        );
        let expected = format!("<{}> {message}", self.username);
        client
            .write_packet(&ServerboundChat {
                message,
                timestamp_millis: now.as_millis().min(i64::MAX as u128) as i64,
                salt: i64::from(self.index),
                signature: None,
                last_seen_offset: 0,
                last_seen_acknowledged: [0; 3],
                last_seen_checksum: 0,
            })
            .await?;
        self.metrics.fixture_input_fences_sent += 1;
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let frame = client.read_frame().await?;
                let acknowledged = if frame.id == ClientboundSystemChat::ID {
                    let packet = ClientboundSystemChat::decode(&mut frame.body.as_ref())?;
                    let tag = mc_nbt::read_network(&mut packet.content_nbt.as_slice())?;
                    literal_component_matches(&tag, &expected)
                } else {
                    false
                };
                self.frame(client, frame, self.view_distance, preparing)
                    .await?;
                if acknowledged {
                    self.metrics.fixture_input_fences_completed += 1;
                    return Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await
        .context("public preparation input echo fence deadline")??;
        Ok(())
    }
    async fn setup_commands(&mut self, client: &mut Client, preparing: bool) -> Result<()> {
        // Preparation alone may response-gate. Measured open-loop demand never
        // calls this path and retains its fixed wall-time denominator.
        let commands = Arc::clone(&self.commands);
        let batches = commands.setup.len().div_ceil(SETUP_COMMAND_BATCH);
        for (index, batch) in commands.setup.chunks(SETUP_COMMAND_BATCH).enumerate() {
            for command in batch {
                client
                    .write_packet(&ServerboundChatCommand {
                        command: command.clone(),
                    })
                    .await?;
                self.metrics.commands_sent += 1;
            }
            if index + 1 == batches {
                // Write receipt must precede receipt of the final save ACK.
                emit(
                    &json!({"event":"fixture_commands_sent","bot":self.index,"commands_sent":commands.setup.len(),
                        "batch_capacity":SETUP_COMMAND_BATCH}),
                )?;
            }
            self.setup_fence(client, &format!("batch_{index}"), preparing)
                .await?;
        }
        if !commands.setup.is_empty() {
            emit(&json!({"event":"fixture_setup_complete","bot":self.index,
                "commands_sent":commands.setup.len(),"fence":"literal own input-ordered chat echo"}))?;
        }
        Ok(())
    }
}
