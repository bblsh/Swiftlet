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

use crate::network::rtc::SocketAddr;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub(super) use quiche::Config;
pub(super) use quiche::Error;

// Real-time Communication Connection Constants
const MAIN_STREAM_ID: u64 = 0; // Bidirectional stream ID# used for reliable communication in the application between the server and the client
                               // This stream is started by the Client when it announces itself to the server when it connects to it

const MAIN_STREAM_PRIORITY: u8 = 100;
const SERVER_REALTIME_START_ID: u64 = 3;
const CLIENT_REALTIME_START_ID: u64 = 2;

pub struct SendBuffer {
    data: Vec<u8>,
    sent: usize,
}

impl SendBuffer {
    pub fn new(data: Vec<u8>) -> Self {
        SendBuffer { data, sent: 0 }
    }
}

// QUIC Connection (Using the quiche crate)
pub(super) struct Connection {
    id: u64,                                     // ID to be used by the application
    current_scid: quiche::ConnectionId<'static>, // Current SCID used by this connection
    connection: quiche::Connection,              // quiche Connection
    recv_info: quiche::RecvInfo,
    last_send_instant: Instant, // Used for sending PING / ACK_Elicting if it's been a while
    next_timeout_instant: Option<Instant>,
    established_once: bool,
    recv_captured: usize,
    recv_target: usize,
    recv_data: Vec<u8>,
    reliable_send_queue: VecDeque<SendBuffer>,
}

pub(super) enum RecvResult {
    Closed(u64),
    Draining(u64),
    Established(u64),
    Nothing,
    ReliableReadTarget(u64),
    Closing(u64),
    StreamReadable((u64, u64)),
}

pub(super) enum TimeoutResult {
    Nothing(Option<Instant>),
    Closed(u64),
    Draining(u64),
    Happened,
}

impl Connection {
    pub(super) fn create_config(
        alpns: &[&[u8]],
        cert_path: &str,
        pkey_path_option: Option<&str>,
        idle_timeout_in_ms: u64,
        max_payload_size: usize,
        reliable_stream_buffer: u64,
        unreliable_stream_buffer: u64,
    ) -> Result<Config, Error> {
        // A quiche Config with default values
        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION)?;

        config.set_application_protos(alpns)?;

        // Do different config things if it is a server vs a client based on pkey path availability
        if let Some(pkey_path) = pkey_path_option {
            // Maybe not return error immediately here?
            config.load_cert_chain_from_pem_file(cert_path)?;
            config.load_priv_key_from_pem_file(pkey_path)?;
            config.verify_peer(false);

            // Enable the ability to log the secret keys for wireshark debugging
            config.log_keys();
        } else {
            // Temporary solution for client to verify certificate
            // Maybe not return error immediately here?
            config.load_verify_locations_from_file(cert_path)?;
            config.verify_peer(true);
        }

        config.set_initial_max_streams_bidi(1);
        config.set_initial_max_streams_uni(1); // Not sure... based on future testing

        config.set_max_idle_timeout(idle_timeout_in_ms);

        config.set_max_recv_udp_payload_size(max_payload_size);
        config.set_max_send_udp_payload_size(max_payload_size);

        // 4_194_304 |  4 MiB
        config.set_initial_max_stream_data_bidi_local(reliable_stream_buffer);
        config.set_initial_max_stream_data_bidi_remote(reliable_stream_buffer);
        config.set_initial_max_stream_data_uni(unreliable_stream_buffer);

        // 16_777_216 | 16 MiB
        config.set_initial_max_data(reliable_stream_buffer + (unreliable_stream_buffer * 4));

        config.enable_pacing(true); // Default that I confirm

        config.set_disable_active_migration(true); // Temporary

        // Enable datagram frames for unreliable data to be sent

        Ok(config)
    }

    #[inline]
    pub(super) fn get_empty_cid() -> [u8; quiche::MAX_CONN_ID_LEN] {
        [0; quiche::MAX_CONN_ID_LEN]
    }

    // returns true if this packet could be a new connection
    pub(super) fn recv_header_analyze(
        data: &mut [u8],
        is_server: bool,
    ) -> Option<(quiche::ConnectionId<'static>, bool)> {
        if let Ok(packet_header) = quiche::Header::from_slice(data, quiche::MAX_CONN_ID_LEN) {
            if is_server
                && packet_header.ty == quiche::Type::Initial
                && quiche::version_is_supported(packet_header.version)
            {
                // This gets reached even when Type is Handshake... look into further
                Some((packet_header.dcid, true))
            } else {
                Some((packet_header.dcid, false))
            }
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        id: u64,
        peer_addr: SocketAddr,
        server_name: Option<&str>,
        local_addr: SocketAddr,
        scid_data: &[u8],
        config: &mut quiche::Config,
        recv_data_capacity: usize,
        writer_opt: Option<Box<std::fs::File>>,
    ) -> Result<Self, Error> {
        let recv_info = quiche::RecvInfo {
            from: local_addr,
            to: local_addr,
        };

        // Do some connectionID length testing here in future
        let scid = quiche::ConnectionId::from_ref(&scid_data[..quiche::MAX_CONN_ID_LEN]);
        let current_scid = scid.into_owned();

        if server_name.is_some() {
            // Create client connection

            let connection =
                quiche::connect(server_name, &current_scid, local_addr, peer_addr, config)?;

            let mut conn_mgr = Connection {
                id,
                current_scid,
                connection,
                recv_info,
                last_send_instant: Instant::now(),
                next_timeout_instant: None,
                established_once: false,
                recv_captured: 0,
                recv_target: 0,
                recv_data: Vec::with_capacity(recv_data_capacity),
                reliable_send_queue: VecDeque::new(),
            };

            conn_mgr.recv_data.resize(recv_data_capacity, 0);

            Ok(conn_mgr)
        } else {
            // Create server connection
            let connection =
                match quiche::accept(&current_scid, None, local_addr, peer_addr, config) {
                    Ok(mut conn) => {
                        if let Some(writer) = writer_opt {
                            // called before recv
                            conn.set_keylog(writer);
                        }
                        conn
                    }
                    Err(err) => {
                        return Err(err);
                    }
                };

            let mut conn_mgr = Connection {
                id,
                current_scid,
                connection,
                recv_info,
                last_send_instant: Instant::now(),
                next_timeout_instant: None,
                established_once: false,
                recv_captured: 0,
                recv_target: 0,
                recv_data: Vec::with_capacity(recv_data_capacity),
                reliable_send_queue: VecDeque::new(),
            };

            conn_mgr.recv_data.resize(recv_data_capacity, 0);

            Ok(conn_mgr)
        }
    }

    #[inline]
    pub(super) fn matches_id(&self, id: u64) -> bool {
        self.id == id
    }

    #[inline]
    pub(super) fn matches_dcid(&self, dcid: &[u8]) -> bool {
        self.current_scid.as_ref() == dcid
    }

    pub(super) fn get_next_send_packet(
        &mut self,
        packet_data: &mut [u8],
    ) -> Result<Option<(usize, SocketAddr, Instant)>, Error> {
        match self.connection.send(packet_data) {
            Ok((packet_len, send_info)) => {
                // let current_instant = Instant::now();
                // if send_info.at > current_instant {
                //     self.last_send_instant = send_info.at;
                // } else {
                //     self.last_send_instant = current_instant;
                // }
                if send_info.at > self.last_send_instant {
                    self.last_send_instant = send_info.at;
                }
                Ok(Some((packet_len, send_info.to, send_info.at)))
            }
            Err(quiche::Error::Done) => {
                self.next_timeout_instant = self.connection.timeout_instant();
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub(super) fn handle_possible_timeout(&mut self) -> TimeoutResult {
        if let Some(timeout_instant) = self.next_timeout_instant {
            let now = Instant::now();
            if timeout_instant <= now {
                // Verifies that a timeout occurred and then processes it
                self.next_timeout_instant = self.connection.timeout_instant();
                if let Some(timeout_verify) = self.next_timeout_instant {
                    if timeout_verify <= now {
                        self.connection.on_timeout();
                        if self.connection.is_closed() {
                            TimeoutResult::Closed(self.id)
                        } else if self.connection.is_draining() {
                            TimeoutResult::Draining(self.id)
                        } else {
                            TimeoutResult::Happened
                        }
                    } else {
                        TimeoutResult::Nothing(self.next_timeout_instant)
                    }
                } else {
                    TimeoutResult::Nothing(self.next_timeout_instant)
                }
            } else {
                TimeoutResult::Nothing(self.next_timeout_instant)
            }
        } else {
            TimeoutResult::Nothing(self.next_timeout_instant)
        }
    }

    fn stream_reliable_send_next(&mut self) -> Result<usize, Error> {
        let mut total_bytes_sent = 0;
        loop {
            if let Some(send_buf) = self.reliable_send_queue.front_mut() {
                match self.connection.stream_send(
                    MAIN_STREAM_ID,
                    &send_buf.data[send_buf.sent..],
                    false,
                ) {
                    Ok(bytes_sent) => {
                        total_bytes_sent += bytes_sent;
                        send_buf.sent += bytes_sent;
                        if send_buf.sent >= send_buf.data.len() {
                            self.reliable_send_queue.pop_front();
                        } else {
                            return Ok(total_bytes_sent);
                        }
                    }
                    Err(Error::Done) => {
                        return Ok(total_bytes_sent);
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            } else {
                return Ok(total_bytes_sent);
            }
        }
    }

    pub(super) fn recv_data_process(
        &mut self,
        data: &mut [u8],
        from_addr: SocketAddr,
    ) -> Result<RecvResult, Error> {
        self.recv_info.from = from_addr;
        let bytes_processed = self.connection.recv(data, self.recv_info)?;
        // Maybe check bytes_processed in future
        if self.established_once {
            if self.connection.is_closed() {
                Ok(RecvResult::Closed(self.id))
            } else if self.connection.is_draining() {
                Ok(RecvResult::Draining(self.id))
            } else {
                self.stream_reliable_send_next()?;

                if let Some(next_readable_stream) = self.connection.stream_readable_next() {
                    if next_readable_stream == MAIN_STREAM_ID {
                        if self.recv_captured >= self.recv_target {
                            Ok(RecvResult::ReliableReadTarget(self.id))
                        } else {
                            let (bytes_read, is_finished) = self.connection.stream_recv(
                                MAIN_STREAM_ID,
                                &mut self.recv_data[self.recv_captured..self.recv_target],
                            )?; // Shouldn't throw a done since it was stated to be readable
                            if !is_finished {
                                self.recv_captured += bytes_read;
                                if self.recv_captured >= self.recv_target {
                                    Ok(RecvResult::ReliableReadTarget(self.id))
                                } else {
                                    Ok(RecvResult::Nothing)
                                }
                            } else {
                                self.connection.close(false, 1, b"Stream0Finished")?;
                                Ok(RecvResult::Closing(self.id))
                            }
                        }
                    } else {
                        Ok(RecvResult::StreamReadable((self.id, next_readable_stream)))
                    }
                } else {
                    Ok(RecvResult::Nothing)
                }
            }
        } else if self.connection.is_established() {
            self.established_once = true;
            Ok(RecvResult::Established(self.id))
        } else if self.connection.is_closed() {
            Ok(RecvResult::Closed(self.id))
        } else if self.connection.is_draining() {
            Ok(RecvResult::Draining(self.id))
        } else {
            Ok(RecvResult::Nothing)
        }
    }

    #[inline]
    pub(super) fn close(&mut self, err: u64, reason: &[u8]) -> Result<bool, Error> {
        self.connection.close(false, err, reason)?;
        Ok(true)
    }

    pub(super) fn send_ping_if_neccessary(&mut self, duration: Duration) -> Result<bool, Error> {
        if self.last_send_instant + duration <= Instant::now() {
            self.connection.send_ack_eliciting()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // #[inline]
    // pub(super) fn create_stream(&mut self, stream_id: u64, urgency: u8) -> Result<bool, Error> {
    //     self.connection.stream_priority(stream_id, urgency, true)?;
    //     Ok(true)
    // }

    #[inline]
    pub(super) fn create_reliable_stream(&mut self) -> Result<bool, Error> {
        self.connection
            .stream_priority(MAIN_STREAM_ID, MAIN_STREAM_PRIORITY, true)?;
        Ok(true)
    }

    pub(super) fn stream_reliable_send(&mut self, data_vec: Vec<u8>) -> Result<usize, Error> {
        let send_buffer = SendBuffer {
            data: data_vec,
            sent: 0,
        };
        self.reliable_send_queue.push_back(send_buffer);
        self.stream_reliable_send_next()
    }

    // Soft errors here for now
    pub(super) fn stream_reliable_next_read_target(&mut self, mut next_target: usize) {
        if next_target > self.recv_data.len() {
            next_target = self.recv_data.len();
        }
        self.recv_captured = 0;
        self.recv_target = next_target;
    }

    pub(super) fn stream_reliable_read(
        &mut self,
        read_copy: &mut [u8],
    ) -> Result<(Option<usize>, Option<Vec<u8>>), Error> {
        if self.recv_captured >= self.recv_target {
            if read_copy.len() >= self.recv_target {
                read_copy[..self.recv_target].copy_from_slice(&self.recv_data[..self.recv_target]);
                Ok((Some(self.recv_target), None))
            } else {
                let capacity = self.recv_data.capacity();
                let mut vec_return =
                    std::mem::replace(&mut self.recv_data, Vec::with_capacity(capacity));
                self.recv_data.resize(capacity, 0);
                vec_return.shrink_to(self.recv_target);
                Ok((None, Some(vec_return)))
            }
        } else {
            match self.connection.stream_recv(
                MAIN_STREAM_ID,
                &mut self.recv_data[self.recv_captured..self.recv_target],
            ) {
                Ok((bytes_read, is_finished)) => {
                    if !is_finished {
                        self.recv_captured += bytes_read;
                        if self.recv_captured >= self.recv_target {
                            if read_copy.len() >= self.recv_target {
                                read_copy[..self.recv_target]
                                    .copy_from_slice(&self.recv_data[..self.recv_target]);
                                Ok((Some(self.recv_target), None))
                            } else {
                                let capacity = self.recv_data.capacity();
                                let mut vec_return = std::mem::replace(
                                    &mut self.recv_data,
                                    Vec::with_capacity(capacity),
                                );
                                self.recv_data.resize(capacity, 0);
                                vec_return.shrink_to(self.recv_target);
                                Ok((None, Some(vec_return)))
                            }
                        } else {
                            Ok((None, None))
                        }
                    } else {
                        self.connection.close(false, 1, b"Stream0Finished")?;

                        // Maybe add closing awareness here later
                        //Ok(RecvResult::Closing(self.id))
                        Ok((None, None))
                    }
                }
                Err(Error::Done) => Ok((None, None)),
                Err(e) => Err(e),
            }
        }
    }

    #[inline]
    pub(super) fn stream_send(
        &mut self,
        stream_id: u64,
        data: &[u8],
        fin: bool,
    ) -> Result<usize, Error> {
        self.connection.stream_send(stream_id, data, fin)
    }

    #[inline]
    pub(super) fn stream_recv(
        &mut self,
        stream_id: u64,
        data: &mut [u8],
    ) -> Result<(usize, bool), Error> {
        self.connection.stream_recv(stream_id, data)
    }
}
