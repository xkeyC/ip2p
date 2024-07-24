mod signal;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use tokio::time::Duration;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::{math_rand_alpha, RTCPeerConnection};
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

#[tokio::main]
async fn main() -> Result<()> {
    // use --call  or --answer
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("usage: {} --call|--answer", args[0]);
        return Ok(());
    }
    if args[1] == "--call" {
        _create_offer_rtc().await?;
    } else if args[1] == "--answer" {
        _create_answer_rtc().await?;
    } else {
        println!("usage: {} --call|--answer", args[0]);
    }
    Ok(())
}

async fn _create_offer_rtc() -> Result<()> {
    let (peer_connection, mut done_rx) = create_webrtc().await?;
    // Create a datachannel with label 'data'
    let data_channel = peer_connection.create_data_channel("data", None).await?;
    // Register channel opening handling
    let d1 = Arc::clone(&data_channel);
    data_channel.on_open(Box::new(move || {
        _on_data_channel(d1)
    }));
    let offer = peer_connection.create_offer(None).await?;
    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    // Sets the LocalDescription, and starts our UDP listeners
    peer_connection.set_local_description(offer).await?;

    // Block until ICE Gathering is complete, disabling trickle ICE
    // we do this because we only can exchange one signaling message
    // in a production application you should exchange ICE Candidates via OnICECandidate
    let _ = gather_complete.recv().await;

    // Output the answer in base64 so we can paste it in browser
    if let Some(local_desc) = peer_connection.local_description().await {
        let json_str = serde_json::to_string(&local_desc)?;
        let b64 = signal::encode(&json_str);
        println!("{b64}");
    } else {
        println!("generate local_description failed!");
    }

    println!("Paste answer below:");
    let line = signal::must_read_stdin()?;
    let desc_data = signal::decode(line.as_str())?;
    let answer = serde_json::from_str::<RTCSessionDescription>(&desc_data)?;
    peer_connection.set_remote_description(answer).await?;
    println!("Press ctrl-c to stop");
    tokio::select! {
        _ = done_rx.recv() => {
            println!("received done signal!");
        }
        _ = tokio::signal::ctrl_c() => {
            println!();
        }
    }
    peer_connection.close().await?;
    Ok(())
}

async fn _create_answer_rtc() -> Result<()> {
    let (peer_connection, mut done_rx) = create_webrtc().await?;
    // Wait for the offer to be pasted
    println!("Paste offer below:");
    let line = signal::must_read_stdin()?;
    println!("Connection...");
    let desc_data = signal::decode(line.as_str())?;
    let offer = serde_json::from_str::<RTCSessionDescription>(&desc_data)?;

    // Set the remote SessionDescription
    peer_connection.set_remote_description(offer).await?;

    // Create an answer
    let answer = peer_connection.create_answer(None).await?;

    // Create channel that is blocked until ICE Gathering is complete
    let mut gather_complete = peer_connection.gathering_complete_promise().await;

    // Sets the LocalDescription, and starts our UDP listeners
    peer_connection.set_local_description(answer).await?;

    // Block until ICE Gathering is complete, disabling trickle ICE
    // we do this because we only can exchange one signaling message
    // in a production application you should exchange ICE Candidates via OnICECandidate
    let _ = gather_complete.recv().await;

    // Output the answer in base64 so we can paste it in browser
    if let Some(local_desc) = peer_connection.local_description().await {
        let json_str = serde_json::to_string(&local_desc)?;
        let b64 = signal::encode(&json_str);
        println!("You offer is ==========\n {b64}\n==========");
    } else {
        println!("generate local_description failed!");
    }

    println!("Press ctrl-c to stop");
    tokio::select! {
        _ = done_rx.recv() => {
            println!("received done signal!");
        }
        _ = tokio::signal::ctrl_c() => {
            println!();
        }
    }

    peer_connection.close().await?;

    Ok(())
}

async fn create_webrtc() -> Result<(Arc<RTCPeerConnection>, tokio::sync::mpsc::Receiver<()>)> {
    let mut m = MediaEngine::default();

    // Register default codecs
    m.register_default_codecs()?;

    // Create a InterceptorRegistry. This is the user configurable RTP/RTCP Pipeline.
    // This provides NACKs, RTCP Reports and other features. If you use `webrtc.NewPeerConnection`
    // this is enabled by default. If you are manually managing You MUST create a InterceptorRegistry
    // for each PeerConnection.
    let mut registry = Registry::new();

    // Use the default set of Interceptors
    registry = register_default_interceptors(registry, &mut m)?;

    // Create the API object with the MediaEngine
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    // Prepare the configuration
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.miwifi.com:3478".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };

    // Create a new RTCPeerConnection
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);
    let (done_tx, done_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Set the handler for Peer connection state
    // This will notify you when the peer has connected/disconnected

    peer_connection.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        println!("Peer Connection State has changed: {s}");

        if s == RTCPeerConnectionState::Failed {
            // Wait until PeerConnection has had no network activity for 30 seconds or another failure. It may be reconnected using an ICE Restart.
            // Use webrtc.PeerConnectionStateDisconnected if you are interested in detecting faster timeout.
            // Note that the PeerConnection may come back from PeerConnectionStateDisconnected.
            println!("Peer Connection has gone to failed exiting");
            let _ = done_tx.try_send(());
        }

        Box::pin(async {})
    }));

    // Register data channel creation handling
    peer_connection
        .on_data_channel(Box::new(move |d: Arc<RTCDataChannel>| {
            _on_data_channel(d)
        }));
    Ok((peer_connection, done_rx))
}

fn _on_data_channel(d: Arc<RTCDataChannel>) -> Pin<Box<dyn Future<Output=()> + Send>> {
    let d_label = d.label().to_owned();
    let d_id = d.id();
    println!("New DataChannel {d_label} {d_id}");

    // Register channel opening handling
    Box::pin(async move {
        let d2 = Arc::clone(&d);
        let d_label2 = d_label.clone();
        let d_id2 = d_id;
        d.on_close(Box::new(move || {
            println!("Data channel closed");
            Box::pin(async {})
        }));

        d.on_open(Box::new(move || {
            println!("Data channel '{d_label2}'-'{d_id2}' open. ");

            Box::pin(async move {
                let mut result = Result::<usize>::Ok(0);
                while result.is_ok() {
                    let timeout = tokio::time::sleep(Duration::from_secs(1));
                    tokio::pin!(timeout);
                    tokio::select! {
                                _ = timeout.as_mut() =>{
                                    // message form stdin
                                    println!("Enter text message to send to DataChannel '{d_label2}'-'{d_id2}'");
                                    let message = signal::must_read_stdin().unwrap_or("".to_owned());
                                    result = d2.send_text(message).await.map_err(Into::into);
                                }
                            }
                }
            })
        }));

        // Register text message handling
        d.on_message(Box::new(move |msg: DataChannelMessage| {
            let msg_str = String::from_utf8(msg.data.to_vec()).unwrap();
            println!("Message from DataChannel '{d_label}': '{msg_str}'");
            Box::pin(async {})
        }))
    })
}