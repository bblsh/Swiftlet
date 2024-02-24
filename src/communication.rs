//Media Enhanced Swiftlet Rust Realtime Media Internet Communications
//MIT License
//Copyright (c) 2024 Jared Loewenthal
//
//Permission is hereby granted, free of charge, to any person obtaining a copy
//of this software and associated documentation files (the "Software"), to deal
//in the Software without restriction, including without limitation the rights
//to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//copies of the Software, and to permit persons to whom the Software is
//furnished to do so, subject to the following conditions:
//
//The above copyright notice and this permission notice shall be included in all
//copies or substantial portions of the Software.
//
//THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//SOFTWARE.

use crossbeam_channel::bounded;
pub(crate) use crossbeam_channel::{Receiver, Sender, TryRecvError};

use crate::audio::OpusData;

pub(crate) struct NetworkThreadChannels {
    pub(crate) command_recv: Receiver<NetworkCommand>,
    pub(crate) network_state_send: Sender<NetworkStateMessage>,
    pub(crate) network_debug_send: Sender<String>, // String so that non-static debug messages can be made!
}

pub(crate) struct ConsoleThreadChannels {
    pub(crate) command_send: Sender<NetworkCommand>,
    pub(crate) network_state_recv: Receiver<NetworkStateMessage>,
    pub(crate) network_debug_recv: Receiver<String>,
}

pub(crate) fn create_networking_console_channels() -> (NetworkThreadChannels, ConsoleThreadChannels)
{
    let (command_send, command_recv) = bounded(64);
    let (network_state_send, network_state_recv) = bounded(64);
    let (network_debug_send, network_debug_recv) = bounded(256);

    let network_channels = NetworkThreadChannels {
        command_recv,
        network_state_send,
        network_debug_send,
    };
    let console_channels = ConsoleThreadChannels {
        command_send,
        network_state_recv,
        network_debug_recv,
    };

    (network_channels, console_channels)
}

pub(crate) enum NetworkCommand {
    Stop(u64),
    Client(ClientCommand),
    Server(ServerCommand),
}

pub(crate) enum ClientCommand {
    StateChange(u8),
    ServerConnect(swiftlet_quic::endpoint::SocketAddr),
    MusicTransfer(OpusData),
}

pub(crate) enum ServerCommand {
    ConnectionClose(usize),
}

pub(crate) enum NetworkStateMessage {
    ServerNameChange(String),
    ConnectionsRefresh((Option<usize>, Vec<NetworkStateConnection>)),
    NewConnection((String, u8)),
    StateChange((usize, u8)),
}

pub(crate) struct NetworkStateConnection {
    pub(crate) name: String,
    pub(crate) state: u8,
}

pub(crate) struct AudioOutputThreadChannels {
    pub(crate) command_recv: Receiver<ConsoleAudioCommands>,
    pub(crate) packet_recv: Receiver<NetworkAudioPackets>,
    pub(crate) state_send: Sender<AudioStateMessage>,
    pub(crate) debug_send: Sender<&'static str>,
}

pub(crate) struct NetworkAudioOutputChannels {
    pub(crate) packet_send: Sender<NetworkAudioPackets>,
}

pub(crate) struct ConsoleAudioOutputChannels {
    pub(crate) command_send: Sender<ConsoleAudioCommands>,
    pub(crate) state_recv: Receiver<AudioStateMessage>,
    pub(crate) debug_recv: Receiver<&'static str>,
}

pub(crate) fn create_audio_output_channels() -> (
    AudioOutputThreadChannels,
    NetworkAudioOutputChannels,
    ConsoleAudioOutputChannels,
) {
    let (audio_output_command_send, audio_output_command_recv) =
        bounded::<ConsoleAudioCommands>(64);
    let (audio_output_packet_send, audio_output_packet_recv) = bounded(64);
    let (audio_output_state_send, audio_output_state_recv) = bounded(64);
    let (audio_output_debug_send, audio_output_debug_recv) = bounded(256);

    let audio_output_channels = AudioOutputThreadChannels {
        command_recv: audio_output_command_recv,
        packet_recv: audio_output_packet_recv,
        state_send: audio_output_state_send,
        debug_send: audio_output_debug_send,
    };
    let network_audio_output_channels = NetworkAudioOutputChannels {
        packet_send: audio_output_packet_send,
    };
    let console_audio_output_channels = ConsoleAudioOutputChannels {
        command_send: audio_output_command_send,
        state_recv: audio_output_state_recv,
        debug_recv: audio_output_debug_recv,
    };

    (
        audio_output_channels,
        network_audio_output_channels,
        console_audio_output_channels,
    )
}

pub(crate) enum ConsoleAudioCommands {
    LoadOpus(OpusData),
    PlayOpus(u64),
}

pub(crate) enum NetworkAudioPackets {
    MusicPacket((u8, Vec<u8>)),
    MusicStop(u8),
    VoiceData(Vec<u8>),
}

pub(crate) enum AudioStateMessage {}
