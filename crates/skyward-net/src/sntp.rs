//! Network time for a recovery whose persisted RTC offset predates the
//! installed system. HTTPS keeps its normal certificate-date checks.

use crate::dns;
use anyhow::{bail, ensure, Context, Result};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const SERVERS: [&str; 2] = ["ntp.aliyun.com", "time.android.com"];
const WAIT: Duration = Duration::from_secs(3);
const UNIX_OFFSET: u64 = 2_208_988_800;

/// The current Unix time in milliseconds, asked independently of the
/// phone's wall clock, using the network's own DNS servers.
pub fn query(servers: &[Ipv4Addr]) -> Result<u64> {
    let mut errors = Vec::new();
    for host in SERVERS {
        let answer = (|| {
            let address = dns::lookup(host, servers)?.into_iter().next().context("time server has no address")?;
            ask(SocketAddr::from((address, 123)), rand::random())
        })();
        match answer {
            Ok(now) => return Ok(now),
            Err(error) => errors.push(format!("{host}: {error:#}")),
        }
    }
    bail!("network time unavailable: {}", errors.join("; "))
}

fn ask(server: SocketAddr, nonce: [u8; 8]) -> Result<u64> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_read_timeout(Some(WAIT))?;
    socket.set_write_timeout(Some(WAIT))?;
    // A connected UDP socket accepts replies from this endpoint alone.
    socket.connect(server)?;
    let mut request = [0u8; 48];
    request[0] = (4 << 3) | 3;
    request[40..48].copy_from_slice(&nonce);
    let sent = Instant::now();
    socket.send(&request)?;
    let mut packet = [0u8; 512];
    let length = socket.recv(&mut packet)?;
    reply(&packet[..length], nonce, sent.elapsed())
}

fn reply(packet: &[u8], nonce: [u8; 8], round_trip: Duration) -> Result<u64> {
    ensure!(packet.len() >= 48, "short network-time reply");
    ensure!(packet[0] >> 6 != 3, "time server is not synchronized");
    ensure!(matches!((packet[0] >> 3) & 7, 3 | 4) && packet[0] & 7 == 4, "not an SNTP server reply");
    ensure!((1..=15).contains(&packet[1]), "time server has no usable stratum");
    ensure!(packet[24..32] == nonce, "network-time reply belongs to another request");
    ensure!(packet[40..48] != [0; 8] && round_trip <= WAIT, "time server supplied no current timestamp");
    let seconds = u64::from(u32::from_be_bytes(packet[40..44].try_into()?));
    let fraction = u64::from(u32::from_be_bytes(packet[44..48].try_into()?));
    // The 32-bit NTP seconds wrap in 2036. These releases cannot predate
    // 2020; timestamps below the Unix offset belong to the next era.
    let seconds = if seconds < UNIX_OFFSET { seconds + (1 << 32) } else { seconds } - UNIX_OFFSET;
    ensure!((1_577_836_800..4_102_444_800).contains(&seconds), "network time lies outside 2020..2100");
    Ok(seconds * 1000 + ((fraction * 1000) >> 32) + round_trip.as_millis() as u64 / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seconds: u64) -> [u8; 48] {
        let mut packet = [0u8; 48];
        packet[0] = (4 << 3) | 4;
        packet[1] = 2;
        packet[24..32].copy_from_slice(&[7; 8]);
        packet[40..44].copy_from_slice(&((seconds + UNIX_OFFSET) as u32).to_be_bytes());
        packet[44..48].copy_from_slice(&0x80000000u32.to_be_bytes());
        packet
    }

    #[test]
    fn server_time_does_not_depend_on_an_incorrect_local_wall_clock() {
        assert_eq!(reply(&packet(1_794_873_600), [7; 8], Duration::from_millis(100)).unwrap(), 1_794_873_600_550);
    }

    #[test]
    fn ntp_seconds_wrapping_in_2036_keep_the_unix_time() {
        assert_eq!(reply(&packet(2_100_000_000), [7; 8], Duration::ZERO).unwrap(), 2_100_000_000_500);
    }

    #[test]
    fn unrelated_or_unsynchronized_replies_are_refused() {
        let packet = packet(1_794_873_600);
        assert!(reply(&packet, [8; 8], Duration::ZERO).is_err());
        for (byte, value) in [(0, 0xe4), (0, 0x23), (0, 0x14), (1, 0), (1, 16)] {
            let mut changed = packet;
            changed[byte] = value;
            assert!(reply(&changed, [7; 8], Duration::ZERO).is_err());
        }
    }

    #[test]
    fn absent_and_obsolete_times_and_long_round_trips_are_refused() {
        let mut empty = packet(1_794_873_600);
        empty[40..48].fill(0);
        assert!(reply(&empty, [7; 8], Duration::ZERO).is_err());
        assert!(reply(&packet(1_500_000_000), [7; 8], Duration::ZERO).is_err());
        assert!(reply(&packet(1_794_873_600), [7; 8], WAIT + Duration::from_millis(1)).is_err());
        assert!(reply(&empty[..47], [7; 8], Duration::ZERO).is_err());
    }

    #[test]
    fn a_udp_exchange_uses_the_requests_random_timestamp() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        server.set_read_timeout(Some(WAIT)).unwrap();
        let at = server.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let mut request = [0u8; 48];
            let (length, from) = server.recv_from(&mut request).unwrap();
            assert_eq!(length, 48);
            assert_eq!(request[0], 0x23);
            let mut answer = packet(1_794_873_600);
            answer[24..32].copy_from_slice(&request[40..48]);
            server.send_to(&answer, from).unwrap();
        });
        let now = ask(at, [3; 8]).unwrap();
        assert!((1_794_873_600_500..=1_794_873_602_000).contains(&now));
        peer.join().unwrap();
    }
}
