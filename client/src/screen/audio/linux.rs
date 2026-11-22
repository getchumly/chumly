//! Screen share audio on Linux, the same under X11 and Wayland: through
//! PipeWire, the sound server of current desktops.
//!
//! We create our own capture node that the session manager does not connect
//! to anything, then link the playback streams we want into it ourselves:
//! window share — only that program and its child processes; screen share —
//! every program except Chumly, so viewers do not hear their own voices.
//! Programs that start playing later are linked as they appear; their links
//! go away with them.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::thread::JoinHandle;

use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;
use pw::types::ObjectType;
use shared::media::{MediaKind, VOICE_SAMPLE_RATE};

use super::{AudioTarget, CHANNELS, Packer};

/// Our capture node's name, as other PipeWire tools show it.
const NODE_NAME: &str = "chumly-share-audio";

pub struct StreamAudio {
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl StreamAudio {
    pub fn start(target: AudioTarget, out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>) -> Result<Self, String> {
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("screen-audio".into())
            .spawn(move || {
                if let Err(e) = run(target, out, quit_rx, &ready_tx) {
                    // If startup already finished, nobody is waiting for this message.
                    let _ = ready_tx.send(Err(e.clone()));
                    tracing::warn!("screen share audio: {e}");
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { quit, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("audio capture thread exited".into()),
        }
    }
}

impl Drop for StreamAudio {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A port of a node, as far as linking needs it.
struct Port {
    node: u32,
    output: bool,
    /// "FL", "FR", "MONO", …
    channel: String,
}

/// A node, as far as linking needs it.
struct Node {
    /// The PipeWire client (connection) that made it: its process is known
    /// from the client, node properties do not carry it.
    client: Option<u32>,
    name: String,
    /// "Stream/Output/Audio" for a program playing sound.
    class: String,
}

/// What the registry told us, and the links we made.
struct Graph {
    target: AudioTarget,
    /// Our own process.
    me: u32,
    /// Process of each client.
    clients: HashMap<u32, u32>,
    nodes: HashMap<u32, Node>,
    ports: HashMap<u32, Port>,
    /// Our links by (their output port, our input port). Dropping a proxy
    /// destroys the link (`object.linger` is off).
    links: HashMap<(u32, u32), pw::link::Link>,
}

impl Graph {
    fn new(target: AudioTarget) -> Self {
        let (clients, nodes, ports, links) = Default::default();
        Self { target, me: std::process::id(), clients, nodes, ports, links }
    }

    /// The process behind a node, if its client is known.
    fn pid(&self, node: &Node) -> Option<u32> {
        node.client.and_then(|c| self.clients.get(&c)).copied()
    }

    /// Links every output port of a wanted stream to our input port of the
    /// same channel (a mono stream to both); drops links whose ports are
    /// gone or whose stream is no longer wanted.
    fn relink(&mut self, core: &pw::core::Core) {
        let ours =
            self.nodes.iter().find(|(_, n)| n.name == NODE_NAME && self.pid(n) == Some(self.me)).map(|(id, _)| *id);
        let wanted: HashSet<u32> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.class == "Stream/Output/Audio" && wanted(self.target, self.pid(n)))
            .map(|(id, _)| *id)
            .collect();
        let ports = &self.ports;
        self.links.retain(|(from, to), _| {
            ports.contains_key(from)
                && ports.contains_key(to)
                && ports.get(from).is_some_and(|p| wanted.contains(&p.node))
        });
        let Some(ours) = ours else { return };
        let inputs: Vec<(u32, &str)> = self
            .ports
            .iter()
            .filter(|(_, p)| p.node == ours && !p.output)
            .map(|(id, p)| (*id, p.channel.as_str()))
            .collect();
        let mut new = Vec::new();
        for (from, port) in &self.ports {
            if !port.output || !wanted.contains(&port.node) {
                continue;
            }
            for &(to, channel) in &inputs {
                let fits = port.channel == channel || port.channel == "MONO";
                if fits && !self.links.contains_key(&(*from, to)) {
                    new.push((*from, port.node, to));
                }
            }
        }
        for (from, node, to) in new {
            let props = pw::properties::properties! {
                "link.output.node" => node.to_string(),
                "link.output.port" => from.to_string(),
                "link.input.node" => ours.to_string(),
                "link.input.port" => to.to_string(),
                "object.linger" => "false",
            };
            match core.create_object::<pw::link::Link>("link-factory", &props) {
                Ok(link) => {
                    self.links.insert((from, to), link);
                }
                Err(e) => tracing::debug!("linking share audio port {from}: {e}"),
            }
        }
    }
}

/// Whether `pid` is `root` or one of its descendants (through /proc).
fn in_tree(pid: u32, root: u32) -> bool {
    let mut cur = pid;
    for _ in 0..64 {
        if cur == root {
            return true;
        }
        if cur <= 1 {
            return false;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{cur}/stat")) else { return false };
        // "pid (comm) state ppid …": comm may contain spaces and parentheses.
        let Some(ppid) = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)) else {
            return false;
        };
        let Ok(ppid) = ppid.parse() else { return false };
        cur = ppid;
    }
    false
}

/// Whether a playback stream of process `pid` (if known) is wanted.
fn wanted(target: AudioTarget, pid: Option<u32>) -> bool {
    match (target, pid) {
        (AudioTarget::Only(root), Some(pid)) => in_tree(pid, root),
        (AudioTarget::Only(_), None) => false,
        (AudioTarget::AllExcept(root), Some(pid)) => !in_tree(pid, root),
        // Unknown process (e.g. some sandboxed apps): not ours, so included.
        (AudioTarget::AllExcept(_), None) => true,
    }
}

fn run(
    target: AudioTarget,
    out: tokio::sync::mpsc::Sender<(MediaKind, Vec<u8>)>,
    quit: pw::channel::Receiver<()>,
    ready: &std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let err = |e: pw::Error| e.to_string();
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(err)?;
    let core = context.connect_rc(None).map_err(|e| format!("PipeWire is not running ({e})"))?;
    let registry = core.get_registry_rc().map_err(err)?;
    let graph = Rc::new(RefCell::new(Graph::new(target)));

    let (g_add, g_remove, core_add, core_remove) = (graph.clone(), graph.clone(), core.clone(), core.clone());
    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| {
            let Some(props) = obj.props else { return };
            let mut g = g_add.borrow_mut();
            match obj.type_ {
                ObjectType::Client => {
                    // Set by PipeWire from the socket's credentials; what the
                    // program says about itself is only a fallback.
                    let pid = props.get("pipewire.sec.pid").or_else(|| props.get("application.process.id"));
                    let Some(pid) = pid.and_then(|p| p.parse().ok()) else { return };
                    g.clients.insert(obj.id, pid);
                }
                ObjectType::Node => {
                    let node = Node {
                        client: props.get("client.id").and_then(|c| c.parse().ok()),
                        name: props.get("node.name").unwrap_or("").to_owned(),
                        class: props.get("media.class").unwrap_or("").to_owned(),
                    };
                    g.nodes.insert(obj.id, node);
                }
                ObjectType::Port => {
                    let (Some(node), Some(dir)) =
                        (props.get("node.id").and_then(|n| n.parse().ok()), props.get("port.direction"))
                    else {
                        return;
                    };
                    let channel = props.get("audio.channel").unwrap_or("").to_owned();
                    g.ports.insert(obj.id, Port { node, output: dir == "out", channel });
                }
                _ => return,
            }
            g.relink(&core_add);
        })
        .global_remove(move |id| {
            let mut g = g_remove.borrow_mut();
            g.clients.remove(&id);
            g.nodes.remove(&id);
            g.ports.remove(&id);
            g.relink(&core_remove);
        })
        .register();

    let stream = pw::stream::StreamBox::new(
        &core,
        NODE_NAME,
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Production",
            *pw::keys::NODE_NAME => NODE_NAME,
            "node.description" => "Chumly: stream audio",
            // We link it ourselves; the session manager must not connect a
            // microphone or anything else to it.
            "node.autoconnect" => "false",
        },
    )
    .map_err(err)?;
    let packer = Packer::new(out)?;
    let _stream_listener = stream
        .add_local_listener_with_user_data(packer)
        .process(|stream, packer| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let (offset, size) = (d.chunk().offset() as usize, d.chunk().size() as usize);
            let Some(bytes) = d.data() else { return };
            let Some(bytes) = bytes.get(offset..offset + size) else { return };
            let samples = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b));
            if let Err(e) = packer.push(samples) {
                tracing::debug!("share audio: {e}");
            }
        })
        .register()
        .map_err(err)?;

    let format = format_param()?;
    let mut params = [Pod::from_bytes(&format).ok_or("bad audio format parameters")?];
    stream
        .connect(spa::utils::Direction::Input, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(err)?;

    let ml = mainloop.clone();
    let _quit = quit.attach(mainloop.loop_(), move |()| ml.quit());
    let _ = ready.send(Ok(()));
    mainloop.run();
    Ok(())
}

/// Interleaved stereo float at 48 kHz: what Opus takes, no conversion.
fn format_param() -> Result<Vec<u8>, String> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(VOICE_SAMPLE_RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let bytes =
        spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
            .map_err(|e| format!("{e:?}"))?
            .0
            .into_inner();
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_tree() {
        let me = std::process::id();
        assert!(in_tree(me, me));
        // We are a descendant of our parent, not the other way round.
        let parent = std::fs::read_to_string(format!("/proc/{me}/stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse::<u32>().ok())
            .unwrap();
        assert!(in_tree(me, parent));
        assert!(!in_tree(parent, me));
    }

    #[test]
    fn which_streams_are_wanted() {
        let me = std::process::id();
        assert!(wanted(AudioTarget::Only(me), Some(me)));
        assert!(!wanted(AudioTarget::Only(me), None));
        assert!(!wanted(AudioTarget::AllExcept(me), Some(me)));
        assert!(wanted(AudioTarget::AllExcept(me), None));
        assert!(wanted(AudioTarget::AllExcept(me), Some(1)));
    }

    /// A 3-second stereo tone as a WAV file, for `pw-play`.
    fn tone_wav() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("chumly-tone-{}.wav", std::process::id()));
        let rate = 48_000u32;
        let samples: Vec<i16> = (0..rate * 3)
            .flat_map(|i| [((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0) as i16; 2])
            .collect();
        let data = samples.len() as u32 * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 0x0002_0001, rate, rate * 4, 0x0010_0004] {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        wav.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        std::fs::write(&path, wav).unwrap();
        path
    }

    /// Another process plays a tone that starts after the capture; its
    /// sound arrives when it is wanted and not otherwise. Needs a running
    /// PipeWire with a session manager and a sink, and `pw-play`.
    #[test]
    #[ignore = "needs a running PipeWire and pw-play"]
    fn captures_another_process() {
        let wav = tone_wav();
        for case in ["all-except-decoy", "only-player", "only-decoy"] {
            let mut decoy = std::process::Command::new("sleep").arg("10").spawn().unwrap();
            // Mostly the player starts after the capture, so it is linked as it
            // appears; "only the player" needs its pid first, so there it is
            // already playing when the capture starts.
            let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
            let mut player = None;
            let audio = match case {
                "all-except-decoy" => StreamAudio::start(AudioTarget::AllExcept(decoy.id()), tx).unwrap(),
                "only-decoy" => StreamAudio::start(AudioTarget::Only(decoy.id()), tx).unwrap(),
                _ => {
                    let p = std::process::Command::new("pw-play").arg(&wav).spawn().unwrap();
                    let a = StreamAudio::start(AudioTarget::Only(p.id()), tx).unwrap();
                    player = Some(p);
                    a
                }
            };
            let mut player = player.unwrap_or_else(|| std::process::Command::new("pw-play").arg(&wav).spawn().unwrap());
            let _ = player.wait();
            drop(audio);
            let _ = decoy.kill();
            let _ = decoy.wait();
            let mut packets = 0;
            while rx.try_recv().is_ok() {
                packets += 1;
            }
            println!("{case}: {packets} packets (20 ms each)");
            if case == "only-decoy" {
                assert!(packets < 5, "{case}: someone else's sound leaked in");
            } else {
                assert!(packets > 50, "{case}: no sound captured");
            }
        }
        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn format_is_valid() {
        let bytes = format_param().unwrap();
        assert!(Pod::from_bytes(&bytes).is_some_and(|p| p.is_object()));
    }
}
