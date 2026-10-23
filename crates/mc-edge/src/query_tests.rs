use super::*;

#[test]
fn serverops_query_challenge_basic_full_and_source_binding() {
    let mut query = Query::new();
    let peer = "127.0.0.1:1234".parse().unwrap();
    let challenge = [0xfe, 0xfd, 9, 1, 2, 3, 4];
    let Some(Request::Challenge(reply)) = query.request(peer, &challenge) else { panic!("challenge response"); };
    assert_eq!(&reply[..5], &[9, 1, 2, 3, 4]);
    let token: i32 = std::str::from_utf8(&reply[5..reply.len()-1]).unwrap().parse().unwrap();
    let mut stat = vec![0xfe, 0xfd, 0, 1, 2, 3, 4];
    stat.extend_from_slice(&token.to_be_bytes());
    assert!(matches!(query.request(peer, &stat), Some(Request::Stat { full: false, .. })));
    assert!(query.request("127.0.0.1:1235".parse().unwrap(), &stat).is_none());
    stat.extend_from_slice(&[0; 4]);
    assert!(matches!(query.request(peer, &stat), Some(Request::Stat { full: true, .. })));
    stat[7] ^= 1;
    assert!(query.request(peer, &stat).is_none());
    for length in 0..7 { assert!(query.request(peer, &challenge[..length]).is_none()); }
}

#[test]
fn serverops_query_stat_wire_is_vanilla_gamespy4() {
    let mut cfg = crate::config::EdgeConfig::default();
    cfg.server.motd = "Test".into();
    let basic = stat(&cfg, 1, &[], false);
    let mut expected = b"\0\0\0\0\0Test\0SMP\0world\x001\x0020\0".to_vec();
    expected.extend_from_slice(&25565u16.to_le_bytes());
    expected.extend_from_slice(b"127.0.0.1\0");
    assert_eq!(basic, expected);
    let full = stat(&cfg, 1, &["Alice".into()], true);
    assert_eq!(&full[5..16], b"splitnum\0\x80\0");
    assert!(full.ends_with(b"\0\x01player_\0\0Alice\0\0"));
    assert!(full.windows(13).any(|w| w == b"numplayers\x001\0"));
}

#[test]
fn serverops_query_expires_challenges_and_bounds_request_work() {
    let mut query = Query::new();
    let peer = "127.0.0.1:1234".parse().unwrap();
    let token = query.token(peer, &[1,2,3,4], 0);
    query.started = Instant::now() - Duration::from_secs(61);
    let mut request = vec![0xfe,0xfd,0,1,2,3,4];
    request.extend_from_slice(&token.to_be_bytes());
    assert!(query.request(peer, &request).is_none());
    for _ in 0..128 { let _ = query.request(peer, &[0xfe,0xfd,9,1,2,3,4]); }
    assert!(query.request(peer, &[0xfe,0xfd,9,1,2,3,4]).is_none());
}

#[tokio::test]
async fn serverops_query_udp_socket_handshake_and_both_stat_forms() {
    let root = tempfile::tempdir().unwrap();
    let server = Arc::new(crate::console::tests::server(root.path()));
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let task = tokio::spawn(serve(socket, Arc::clone(&server)));
    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(address).await.unwrap();
    client.send(&[0xfe, 0xfd, 9, 1, 2, 3, 4]).await.unwrap();
    let mut buffer = [0; 2048];
    let length = tokio::time::timeout(Duration::from_secs(5), client.recv(&mut buffer)).await.unwrap().unwrap();
    let token: i32 = std::str::from_utf8(&buffer[5..length-1]).unwrap().parse().unwrap();
    let mut request = vec![0xfe, 0xfd, 0, 1, 2, 3, 4];
    request.extend_from_slice(&token.to_be_bytes());
    for full in [false, true] {
        if full { request.extend_from_slice(&[0; 4]); }
        client.send(&request).await.unwrap();
        let length = tokio::time::timeout(Duration::from_secs(5), client.recv(&mut buffer)).await.unwrap().unwrap();
        assert_eq!(&buffer[..5], &[0, 1, 2, 3, 4]);
        let expected = stat(&server.cfg, 0, &[], full);
        assert_eq!(&buffer[5..length], &expected[5..]);
    }
    task.abort();
    server.hub.shutdown();
}
