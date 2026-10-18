// MIX-08 uses a local relay because wire_client::Client owns its socket privately.
// Only selected readers use it. Server-to-client socket reads are capped before
// decoding; client-to-server writes (including keepalives/respawn) stay independent.
const READER_CHUNK: usize = 4096;

struct ReaderBudget {
    rate: Option<u64>,
    ready: tokio::time::Instant,
}
impl ReaderBudget {
    fn new(rate: Option<u64>, now: tokio::time::Instant) -> Self {
        Self { rate, ready: now }
    }
    fn capacity(&self) -> usize {
        self.rate.map_or(READER_CHUNK, |rate| {
            rate.div_ceil(100).clamp(1, READER_CHUNK as u64) as usize
        })
    }
    fn received(&mut self, bytes: usize, now: tokio::time::Instant) {
        self.ready = self.rate.map_or(now, |rate| {
            now + Duration::from_secs_f64(bytes as f64 / rate as f64)
        });
    }
}

async fn copy_reader<R, W>(
    mut source: R,
    mut destination: W,
    mut control: watch::Receiver<Option<u64>>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut budget = ReaderBudget::new(*control.borrow_and_update(), tokio::time::Instant::now());
    let mut buffer = [0u8; READER_CHUNK];
    loop {
        tokio::select! {
            biased;
            changed = control.changed() => {
                changed.context("reader control closed")?;
                budget = ReaderBudget::new(*control.borrow_and_update(), tokio::time::Instant::now());
                continue;
            }
            _ = tokio::time::sleep_until(budget.ready) => {}
        }
        let capacity = budget.capacity();
        let bytes = tokio::select! {
            biased;
            changed = control.changed() => {
                changed.context("reader control closed")?;
                budget = ReaderBudget::new(*control.borrow_and_update(), tokio::time::Instant::now());
                continue;
            }
            bytes = source.read(&mut buffer[..capacity]) => bytes?,
        };
        if bytes == 0 {
            destination.shutdown().await?;
            return Ok(());
        }
        budget.received(bytes, tokio::time::Instant::now());
        destination.write_all(&buffer[..bytes]).await?;
    }
}

impl Bot {
    async fn connect_reader(&mut self, address: SocketAddr) -> Result<Client> {
        let limited = self.mechanics.as_ref().is_some_and(|shared| {
            shared.lock().spec.actions.iter().any(|action| {
                action.bot == self.index
                    && matches!(action.kind.as_str(), "reader_rate_limit" | "restore_reader")
            })
        });
        if !limited {
            return Client::connect(address).await;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local_address = listener.local_addr()?;
        let upstream = tokio::net::TcpStream::connect(address).await?;
        upstream.set_nodelay(true)?;
        let client = Client::connect(local_address).await?;
        let (downstream, _) = listener.accept().await?;
        downstream.set_nodelay(true)?;
        let (control, receiver) = watch::channel(None);
        self.reader_control = Some(control);
        self.reader_relay = Some(tokio::spawn(async move {
            let (server_read, mut server_write) = upstream.into_split();
            let (mut client_read, client_write) = downstream.into_split();
            tokio::select! {
                _ = tokio::io::copy(&mut client_read, &mut server_write) => {}
                _ = copy_reader(server_read, client_write, receiver) => {}
            }
        }));
        Ok(client)
    }
}

impl Drop for Bot {
    fn drop(&mut self) {
        if let Some(relay) = self.reader_relay.take() {
            relay.abort();
        }
    }
}

#[cfg(test)]
mod reader_tests {
    use super::*;
    use std::task::Poll;
    use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

    #[test]
    fn reader_budget_bounds_bursts_and_accounts_actual_bytes() {
        let now = tokio::time::Instant::now();
        let mut budget = ReaderBudget::new(Some(1000), now);
        assert_eq!(budget.capacity(), 10);
        budget.received(3, now);
        assert_eq!(budget.ready - now, Duration::from_millis(3));
        let mut slow = ReaderBudget::new(Some(1), now);
        assert_eq!(slow.capacity(), 1);
        slow.received(1, now);
        assert_eq!(slow.ready - now, Duration::from_secs(1));
        assert_eq!(
            ReaderBudget::new(Some(u64::MAX), now).capacity(),
            READER_CHUNK
        );
        let mut restored = ReaderBudget::new(None, now);
        assert_eq!(restored.capacity(), READER_CHUNK);
        restored.received(READER_CHUNK, now);
        assert_eq!(restored.ready, now);
    }

    #[tokio::test]
    async fn restoring_reader_wakes_rate_wait_and_drains_remaining_bytes() -> Result<()> {
        let (sender, receiver) = watch::channel(Some(1));
        let (destination, mut observed) = tokio::io::duplex(64);
        let task = tokio::spawn(copy_reader(
            std::io::Cursor::new(vec![11u8, 22, 33, 44]),
            destination,
            receiver,
        ));
        let mut first = [0u8; 1];
        tokio::time::timeout(Duration::from_secs(1), observed.read_exact(&mut first)).await??;
        assert_eq!(first, [11]);
        // Poll the destination, never sleep: the next byte must still be gated.
        let mut byte = [0u8; 1];
        let pending = std::future::poll_fn(|cx| {
            let mut buffer = ReadBuf::new(&mut byte);
            Poll::Ready(matches!(
                std::pin::Pin::new(&mut observed).poll_read(cx, &mut buffer),
                Poll::Pending
            ))
        })
        .await;
        assert!(pending);
        sender.send(None)?;
        let mut remaining = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), observed.read_to_end(&mut remaining))
            .await??;
        assert_eq!(remaining, [22, 33, 44]);
        task.await??;
        Ok(())
    }
}
