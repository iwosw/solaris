use super::*;
use mc_protocol::packets::ResourcePackStatus;

async fn exchange(required: bool, action: ResourcePackAction, wrong_id: bool) -> bool {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stream = tokio::net::TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (other, _) = listener.accept().await.unwrap();
    let mut client = Connection::new(stream);
    let mut server = Connection::new(other);
    let task = tokio::spawn(async move {
        let pack = ResourcePackSection { url: "https://example.org/pack.zip".into(), required, prompt: "{\"text\":\"Install pack\"}".into(), ..Default::default() };
        negotiate(&mut server, &pack, &mut ClientPrefs::default()).await
    });
    let frame = client.read_frame_in(mc_protocol::State::Configuration).await.unwrap();
    assert_eq!(frame.id, ResourcePackPush::ID);
    let push = ResourcePackPush::decode(&mut frame.body.clone()).unwrap();
    assert_eq!(push.required, required);
    assert!(push.prompt.is_some());
    let id = if wrong_id { uuid::Uuid::nil() } else { push.id };
    for action in [ResourcePackAction::Accepted, action] {
        client.send_packet(&ServerboundResourcePack { status: ResourcePackStatus { id, action } }).await.unwrap();
        if wrong_id { break; }
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task).await.unwrap().unwrap();
    if result.is_err() {
        let frame = client.read_frame_in(mc_protocol::State::Configuration).await.unwrap();
        assert_eq!(frame.id, ConfigurationDisconnect::ID);
    }
    result.is_ok()
}

#[tokio::test]
async fn serverops_pack_accept_decline_failure_and_identity() {
    assert!(exchange(true, ResourcePackAction::SuccessfullyLoaded, false).await);
    assert!(exchange(false, ResourcePackAction::Declined, false).await);
    assert!(!exchange(true, ResourcePackAction::Declined, false).await);
    assert!(!exchange(true, ResourcePackAction::FailedDownload, false).await);
    assert!(!exchange(true, ResourcePackAction::SuccessfullyLoaded, true).await);
}

#[test]
fn serverops_pack_wire_matches_local_oracle_layout() {
    let packet = ResourcePackPush { id: uuid::Uuid::nil(), url: "u".into(), sha1: "h".into(), required: true, prompt: None };
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();
    assert_eq!(ResourcePackPush::ID, 9);
    assert_eq!(&encoded[..16], &[0; 16]);
    assert_eq!(&encoded[16..], &[1, b'u', 1, b'h', 1, 0]);
    assert_eq!(ResourcePackPush::decode(&mut bytes::Bytes::from(encoded)).unwrap(), packet);
}

#[test]
fn serverops_pack_config_rejects_bad_urls_hashes_and_prompts() {
    let mut pack = ResourcePackSection { url: "file:///private.zip".into(), ..Default::default() };
    assert!(validate(&pack).is_err());
    pack.url = "https://example.org/pack.zip".into();
    pack.sha1 = "not-a-hash".into();
    assert!(validate(&pack).is_err());
    pack.sha1 = "a".repeat(40);
    pack.prompt = "not json".into();
    assert!(validate(&pack).is_err());
    pack.prompt = "{\"text\":\"Install\",\"bold\":true,\"extra\":[\" now\"]}".into();
    assert!(validate(&pack).is_ok());
}
