impl<I> Action<I> for BroadcastNextPayload {
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        async move {
            let payload = env
                .latest_payload_executed
                .clone()
                .ok_or_else(|| eyre!("No latest_payload_executed in env"))?;

            let mut valid_found = false;

            for client in &env.node_clients {
                let result = client
                    .new_payload_v3_wait(
                        payload.clone(),
                        self.versioned_hashes.clone(),
                        self.parent_beacon_block_root,
                    )
                    .await;

                match result {
                    Ok(status) if status.is_valid() => {
                        env.latest_payload_executed = Some(payload.clone());
                        valid_found = true;
                        break;
                    }
                    Ok(status) => {
                        tracing::warn!(?status, "Client did not return valid payload");
                    }
                    Err(err) => {
                        tracing::error!(?err, "Error during payload broadcast");
                    }
                }
            }

            if !valid_found {
                return Err(eyre!("No client responded with a valid payload"));
            }

            Ok(())
        }
        .boxed()
    }
}
