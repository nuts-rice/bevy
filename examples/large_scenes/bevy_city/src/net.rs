use std::collections::HashMap;

use bevy::prelude::*;
use moq_mux::catalog;
use tokio::sync::mpsc;
use url::Url;

use crate::input::{handle_viewers, Command};
use crate::stats::Stats;
use crate::status::StatusPublisher;
use crate::video::VideoEncoder;

use crate::generate_city::GalleryScene;
use bevy::render::gpu_readback::ReadbackComplete;

// Buttons pressed per-viewer per-scene
#[derive(Resource, Default)]
pub struct GalleryViewers(pub HashMap<u64, HashMap<String, Vec<crate::input::Button>>>);

pub struct SceneNet {
    pub encoder: VideoEncoder,
    pub status: StatusPublisher,
    pub commands: mpsc::Receiver<Command>,
    pub was_watched: bool,
    pub catalog: moq_mux::catalog::Producer,
    _broadcast: moq_net::broadcast::Producer,
    pub stats: Stats,
}

#[derive(Resource)]
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
        let encoder = rt.block_on(VideoEncoder::spawn(broadcast.clone(), catalog.clone()));
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
                stats: Stats::new(),
                catalog,
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

pub fn shutdown_net(mut net: ResMut<Net>) {
    for (_, mut scene) in net.scenes.drain() {
        scene.encoder.finish();
        scene.status.finish();
        if let Err(e) = scene.catalog.finish() {
            tracing::warn!(error = %e, "error finishing catalog");
        }
        scene._broadcast.close()
    }
}

pub fn on_gallery_frame(
    frame: On<ReadbackComplete>,
    scenes: Query<&GalleryScene>,
    net: Res<Net>,
    time: Res<Time<Real>>,
) {
    let Ok(scene) = scenes.get(frame.entity) else {
        return;
    };
    let Some(city) = net.scenes.get(&scene.0) else {
        return;
    };
    if !city.encoder.demand.is_used() {
        return;
    }
    let Ok(timestamp) = moq_net::Timestamp::try_from(time.elapsed()) else {
        return;
    };
    city.encoder
        .try_frame(bytes::Bytes::from(frame.data.clone()), timestamp);
}

// Render only what someone is watching.
pub fn gate_gallery_cameras(
    mut net: ResMut<Net>,
    mut cameras: Query<(&GalleryScene, &mut Camera)>,
) {
    for (scene, mut camera) in &mut cameras {
        let Some(city) = net.scenes.get_mut(&scene.0) else {
            return;
        };
        let watched = city.encoder.demand.is_used();
        if watched && !city.was_watched {
            city.encoder.force_keyframe();
        }
        city.was_watched = watched;
        camera.is_active = watched;
    }
}

pub fn drain_viewer_commands(mut net: ResMut<Net>, mut viewers: ResMut<GalleryViewers>) {
    for (&seed, s) in net.scenes.iter_mut() {
        let state = viewers.0.entry(seed).or_default();
        while let Ok(cmd) = s.commands.try_recv() {
            match cmd {
                Command::Buttons {
                    buttons, viewer_id, ..
                } => {
                    state.insert(viewer_id, buttons);
                }
                Command::ViewerLeft { viewer_id } => {
                    state.remove(&viewer_id);
                }
                Command::Reset => state.clear(),
            }
            let mut buttons: Vec<_> = state.values().flatten().copied().collect();
            buttons.sort_by_key(|b| *b as u8);
            buttons.dedup();
            s.status.publish(&crate::status::Status {
                buttons,
                latency: Default::default(),
                stats: s.stats.report(),
                location: None,
            });
        }
    }
}
