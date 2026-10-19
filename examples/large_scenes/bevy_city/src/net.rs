use std::collections::HashMap;

use bevy::prelude::*;
use tokio::sync::mpsc;
use url::Url;

use bevy::prelude::*;

use crate::input::{self, handle_viewers, Command};
use crate::status::StatusPublisher;
use crate::video::VideoEncoder;

pub struct SceneNet {
    pub encoder: VideoEncoder,
    pub status: StatusPublisher,
    pub commands: mpsc::Receiver<Command>,
    pub was_watched: bool,
    _broadcast: moq_net::broadcast::Producer,
}

pub struct Net {
    pub scenes: HashMap<u64, SceneNet>,
    _conn: moq_tokio::Connection,
    _rt: tokio::runtime::Runtime,
}

pub fn connect(relay: Url, seeds: &[u64]) -> anyhow::Result<Net> {
    let _ = moq_tokio::crypto::install_default();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("moq")
        .build()?;
    let _guard = rt.enter();
    let publish = moq_tokio::origin::spawn();
    let subscribe = moq_tokio::origin::spawn();
    let viewers_only = moq_net::Patterns::from(moq_net::Pattern::subtree("viewer")?);
    let sub_scoped = subscribe.scope("", &viewers_only)?;

    let client = moq_tokio::connect::Config::default()
        .init(moq_tokio::quic::Config::default())?
        .with_publisher(publish.consume())
        .with_subscriber(sub_scoped);
    let conn = client.connect(relay.clone());
    tracing::info!(relay = %moq_tokio::RedactedUrl::new(&relay), "dialing relay");

    let mut scenes = HashMap::new();
    // whos gonna be sending commands in command track
    let mut senders = HashMap::new();
    for seed in seeds {
        // Create tracks , fill them in , then announce
        let mut broadcast = publish.create_broadcast(format!("city/{seed}"))?;
        let catalog =
            moq_mux::catalog::Producer::new(&mut broadcast, moq_mux::catalog::Config::default())?;
        let status = StatusPublisher::new(&mut broadcast)?;
        let encoder = rt.block_on(VideoEncoder::spawn(broadcast.clone(), catalog));
        broadcast.announce(Default::default())?;
        let (tx, rx) = mpsc::channel(64);
        senders.insert(seed.to_string(), tx);
        scenes.insert(
            *seed,
            SceneNet {
                encoder,
                status,
                commands: rx,
                was_watched: false,
                _broadcast: broadcast,
            },
        );
    }
    let viewer_origin = subscribe
        .consume()
        .scope("viewer", &moq_net::Patterns::from(moq_net::Pattern::all()))?;
    rt.spawn(async move {
        if let Err(e) = handle_viewers(&viewer_origin, &senders).await {
            tracing::error!(error = %e, "viewer announce loop ended");
        }
    });
    drop(_guard);

    Ok(Net {
        scenes,
        _conn: conn,
        _rt: rt,
    })
}
