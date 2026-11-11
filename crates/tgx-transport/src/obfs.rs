//! Obfuscated MTProto transport handshake (the 64-byte init) and the
//! re-encryption between a TDLib client using a `dd` secret and Telegram's
//! standard (secret-less) obfuscation on the WebSocket side.

use aes::Aes256;
use ctr::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use sha2::{Digest, Sha256};

type Ctr = ctr::Ctr128BE<Aes256>;

pub const HANDSHAKE_LEN: usize = 64;
const SKIP_LEN: usize = 8;
const PREKEY_LEN: usize = 32;
const IV_LEN: usize = 16;
const PREKEY_IV_LEN: usize = PREKEY_LEN + IV_LEN;
const PROTO_TAG_POS: usize = 56;
const DC_IDX_POS: usize = 60;
const TEST_DC_OFFSET: u16 = 10_000;

/// MTProto transport flavour announced in the init packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtoTag {
    Abridged,
    Intermediate,
    PaddedIntermediate,
}

impl ProtoTag {
    pub fn bytes(self) -> [u8; 4] {
        match self {
            ProtoTag::Abridged => [0xef; 4],
            ProtoTag::Intermediate => [0xee; 4],
            ProtoTag::PaddedIntermediate => [0xdd; 4],
        }
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        [
            ProtoTag::Abridged,
            ProtoTag::Intermediate,
            ProtoTag::PaddedIntermediate,
        ]
        .into_iter()
        .find(|t| t.bytes() == b)
    }
}

/// A successfully decoded client init. `dc` is already normalised: test DCs
/// (index ≥ 10000) have the offset removed and `test_dc` set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientInit {
    pub dc: u16,
    pub media: bool,
    pub test_dc: bool,
    pub proto: ProtoTag,
    prekey_iv: [u8; PREKEY_IV_LEN],
}

impl ClientInit {
    /// Signed DC index to announce upstream: negative for media DCs, without
    /// the test-DC offset (the test environment is selected by the WS path).
    pub fn dc_idx(&self) -> i16 {
        // dc came from an i16 magnitude, so it fits.
        let dc = self.dc as i16;
        if self.media { -dc } else { dc }
    }
}

fn sha256_key(prekey: &[u8], secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(prekey);
    h.update(secret);
    h.finalize().into()
}

fn cipher(key: &[u8], iv: &[u8]) -> Ctr {
    Ctr::new_from_slices(key, iv).expect("fixed key/iv lengths")
}

fn reversed(b: &[u8]) -> [u8; PREKEY_IV_LEN] {
    let mut r = [0u8; PREKEY_IV_LEN];
    r.copy_from_slice(b);
    r.reverse();
    r
}

/// Decodes the client's 64-byte init with the proxy secret. `None` means a
/// wrong secret, an unknown transport, or not MTProto at all.
pub fn parse_client_init(init: &[u8; HANDSHAKE_LEN], secret: &[u8; 16]) -> Option<ClientInit> {
    let mut prekey_iv = [0u8; PREKEY_IV_LEN];
    prekey_iv.copy_from_slice(&init[SKIP_LEN..SKIP_LEN + PREKEY_IV_LEN]);
    let key = sha256_key(&prekey_iv[..PREKEY_LEN], secret);
    let mut plain = *init;
    cipher(&key, &prekey_iv[PREKEY_LEN..]).apply_keystream(&mut plain);

    let proto = ProtoTag::from_bytes(&plain[PROTO_TAG_POS..PROTO_TAG_POS + 4])?;
    let idx = i16::from_le_bytes([plain[DC_IDX_POS], plain[DC_IDX_POS + 1]]);
    let mut dc = idx.unsigned_abs();
    let test_dc = dc >= TEST_DC_OFFSET;
    if test_dc {
        dc -= TEST_DC_OFFSET;
    }
    Some(ClientInit {
        dc,
        media: idx < 0,
        test_dc,
        proto,
        prekey_iv,
    })
}

fn is_reserved(b: &[u8; HANDSHAKE_LEN]) -> bool {
    const RESERVED_STARTS: [[u8; 4]; 7] = [
        *b"HEAD",
        *b"POST",
        *b"GET ",
        [0xee; 4],
        [0xdd; 4],
        [0x16, 0x03, 0x01, 0x02],
        *b"OPTI",
    ];
    b[0] == 0xef || RESERVED_STARTS.iter().any(|s| b[..4] == *s) || b[4..8] == [0; 4]
}

/// Builds a fresh standard-obfuscation init for the upstream (Telegram) side.
pub fn generate_relay_init(proto: ProtoTag, dc_idx: i16) -> [u8; HANDSHAKE_LEN] {
    let mut init = [0u8; HANDSHAKE_LEN];
    loop {
        rand::fill(&mut init[..]);
        if !is_reserved(&init) {
            break;
        }
    }
    let mut tail = [0u8; 8];
    tail[..4].copy_from_slice(&proto.bytes());
    tail[4..6].copy_from_slice(&dc_idx.to_le_bytes());
    rand::fill(&mut tail[6..]);

    let mut keystream = [0u8; HANDSHAKE_LEN];
    cipher(
        &init[SKIP_LEN..SKIP_LEN + PREKEY_LEN],
        &init[SKIP_LEN + PREKEY_LEN..PROTO_TAG_POS],
    )
    .apply_keystream(&mut keystream);
    for i in 0..8 {
        init[PROTO_TAG_POS + i] = tail[i] ^ keystream[PROTO_TAG_POS + i];
    }
    init
}

/// The four AES-256-CTR streams of one relayed connection.
pub struct CryptoCtx {
    up: UpCipher,
    down: DownCipher,
}

/// Client -> Telegram direction (`clt_dec` then `tg_enc`).
pub struct UpCipher {
    clt_dec: Ctr,
    tg_enc: Ctr,
}

/// Telegram -> client direction (`tg_dec` then `clt_enc`).
pub struct DownCipher {
    tg_dec: Ctr,
    clt_enc: Ctr,
}

impl CryptoCtx {
    pub fn new(client: &ClientInit, secret: &[u8; 16], relay_init: &[u8; HANDSHAKE_LEN]) -> Self {
        let pi = &client.prekey_iv;
        let mut clt_dec = cipher(&sha256_key(&pi[..PREKEY_LEN], secret), &pi[PREKEY_LEN..]);
        clt_dec.seek(HANDSHAKE_LEN as u64);
        let r = reversed(pi);
        let clt_enc = cipher(&sha256_key(&r[..PREKEY_LEN], secret), &r[PREKEY_LEN..]);

        let relay_pi = &relay_init[SKIP_LEN..SKIP_LEN + PREKEY_IV_LEN];
        let mut tg_enc = cipher(&relay_pi[..PREKEY_LEN], &relay_pi[PREKEY_LEN..]);
        tg_enc.seek(HANDSHAKE_LEN as u64);
        let r = reversed(relay_pi);
        let tg_dec = cipher(&r[..PREKEY_LEN], &r[PREKEY_LEN..]);

        CryptoCtx {
            up: UpCipher { clt_dec, tg_enc },
            down: DownCipher { tg_dec, clt_enc },
        }
    }

    /// See [`UpCipher::client_to_tg`].
    pub fn client_to_tg(&mut self, buf: &mut [u8]) -> Vec<u8> {
        self.up.client_to_tg(buf)
    }

    /// See [`DownCipher::tg_to_client`].
    pub fn tg_to_client(&mut self, buf: &mut [u8]) {
        self.down.tg_to_client(buf)
    }

    /// Splits into independent per-direction halves for concurrent copying.
    pub fn split(self) -> (UpCipher, DownCipher) {
        (self.up, self.down)
    }
}

impl UpCipher {
    /// Decrypts client bytes in place (leaving plaintext in `buf`, which the
    /// packet splitter inspects) and returns them re-encrypted for Telegram.
    pub fn client_to_tg(&mut self, buf: &mut [u8]) -> Vec<u8> {
        self.clt_dec.apply_keystream(buf);
        let mut out = buf.to_vec();
        self.tg_enc.apply_keystream(&mut out);
        out
    }
}

impl DownCipher {
    /// Re-encrypts Telegram bytes for the client, in place.
    pub fn tg_to_client(&mut self, buf: &mut [u8]) {
        self.tg_dec.apply_keystream(buf);
        self.clt_enc.apply_keystream(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::Aes256;
    use ctr::cipher::{KeyIvInit, StreamCipher};
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use sha2::{Digest, Sha256};

    type Ctr = ctr::Ctr128BE<Aes256>;

    const SECRET: [u8; 16] = [7; 16];

    fn ctr(key: &[u8], iv: &[u8]) -> Ctr {
        Ctr::new_from_slices(key, iv).unwrap()
    }

    fn sha_key(prekey: &[u8], secret: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(prekey);
        h.update(secret);
        h.finalize().into()
    }

    fn reserved(b: &[u8; 64]) -> bool {
        const STARTS: [[u8; 4]; 7] = [
            *b"HEAD",
            *b"POST",
            *b"GET ",
            [0xee; 4],
            [0xdd; 4],
            [0x16, 0x03, 0x01, 0x02],
            *b"OPTI",
        ];
        b[0] == 0xef || STARTS.iter().any(|s| b[..4] == *s) || b[4..8] == [0; 4]
    }

    /// What TDLib does for an MTProto proxy with a `dd` secret: the client
    /// side of the obfuscated handshake plus its continued streams.
    struct FakeClient {
        init: [u8; 64],
        enc: Ctr,
        dec: Ctr,
    }

    fn client_side(
        rng: &mut StdRng,
        secret: &[u8; 16],
        proto: ProtoTag,
        dc_idx: i16,
    ) -> FakeClient {
        let mut init = [0u8; 64];
        loop {
            rng.fill_bytes(&mut init);
            if !reserved(&init) {
                break;
            }
        }
        let mut enc = ctr(&sha_key(&init[8..40], secret), &init[40..56]);
        let mut full = init;
        enc.apply_keystream(&mut full);
        let mut tail = [0u8; 8];
        tail[..4].copy_from_slice(&proto.bytes());
        tail[4..6].copy_from_slice(&dc_idx.to_le_bytes());
        tail[6..].copy_from_slice(&[0x11, 0x22]);
        for i in 0..8 {
            init[56 + i] ^= tail[i] ^ full[56 + i];
        }
        let mut rev = [0u8; 48];
        rev.copy_from_slice(&init[8..56]);
        rev.reverse();
        let dec = ctr(&sha_key(&rev[..32], secret), &rev[32..48]);
        FakeClient { init, enc, dec }
    }

    fn parsed(proto: ProtoTag, dc_idx: i16) -> ClientInit {
        let mut rng = StdRng::seed_from_u64(1);
        let c = client_side(&mut rng, &SECRET, proto, dc_idx);
        parse_client_init(&c.init, &SECRET).expect("valid init")
    }

    #[test]
    fn parses_dc_media_and_proto() {
        let a = parsed(ProtoTag::Abridged, 2);
        assert_eq!(
            (a.dc, a.media, a.test_dc, a.proto),
            (2, false, false, ProtoTag::Abridged)
        );
        let b = parsed(ProtoTag::Intermediate, -4);
        assert_eq!(
            (b.dc, b.media, b.test_dc, b.proto),
            (4, true, false, ProtoTag::Intermediate)
        );
        let c = parsed(ProtoTag::PaddedIntermediate, 10002);
        assert_eq!(
            (c.dc, c.media, c.test_dc, c.proto),
            (2, false, true, ProtoTag::PaddedIntermediate)
        );
    }

    #[test]
    fn dc_203_kept_as_203() {
        assert_eq!(parsed(ProtoTag::Intermediate, 203).dc, 203);
    }

    #[test]
    fn wrong_secret_rejected() {
        let mut rng = StdRng::seed_from_u64(2);
        for _ in 0..100 {
            let c = client_side(&mut rng, &SECRET, ProtoTag::Intermediate, 2);
            assert_eq!(parse_client_init(&c.init, &[8; 16]), None);
        }
    }

    #[test]
    fn relay_init_avoids_reserved_prefixes() {
        for _ in 0..10_000 {
            let init = generate_relay_init(ProtoTag::Abridged, 2);
            assert!(!reserved(&init), "reserved prefix in {init:02x?}");
        }
    }

    #[test]
    fn relay_init_decodes_with_standard_obfuscation() {
        for (proto, dc_idx) in [
            (ProtoTag::Abridged, 2i16),
            (ProtoTag::PaddedIntermediate, -5),
        ] {
            let init = generate_relay_init(proto, dc_idx);
            let mut plain = init;
            ctr(&init[8..40], &init[40..56]).apply_keystream(&mut plain);
            assert_eq!(plain[56..60], proto.bytes());
            assert_eq!(i16::from_le_bytes([plain[60], plain[61]]), dc_idx);
        }
    }

    #[test]
    fn roundtrip_both_directions() {
        let mut rng = StdRng::seed_from_u64(3);
        let mut client = client_side(&mut rng, &SECRET, ProtoTag::Intermediate, -2);
        let init = parse_client_init(&client.init, &SECRET).unwrap();
        let relay_init = generate_relay_init(init.proto, init.dc_idx());
        let mut ctx = CryptoCtx::new(&init, &SECRET, &relay_init);

        // Telegram's view of the relay side (standard obfuscation, no secret).
        let mut tg_dec = ctr(&relay_init[8..40], &relay_init[40..56]);
        let mut skip = [0u8; 64];
        tg_dec.apply_keystream(&mut skip);
        let mut rev = [0u8; 48];
        rev.copy_from_slice(&relay_init[8..56]);
        rev.reverse();
        let mut tg_enc = ctr(&rev[..32], &rev[32..48]);

        for round in 0..3u8 {
            let p: Vec<u8> = (0..100 + round as usize * 37)
                .map(|i| i as u8 ^ round)
                .collect();
            let mut wire = p.clone();
            client.enc.apply_keystream(&mut wire);
            let mut out = ctx.client_to_tg(&mut wire);
            assert_eq!(wire, p, "plaintext left in buf");
            tg_dec.apply_keystream(&mut out);
            assert_eq!(out, p);

            let q: Vec<u8> = (0..50 + round as usize * 11)
                .map(|i| (i as u8).wrapping_mul(3))
                .collect();
            let mut back = q.clone();
            tg_enc.apply_keystream(&mut back);
            ctx.tg_to_client(&mut back);
            client.dec.apply_keystream(&mut back);
            assert_eq!(back, q);
        }
    }

    /// Vector produced by upstream `_try_handshake` (tg-ws-proxy @ 6690b78):
    /// the init below was built for secret 00..0f, proto intermediate,
    /// dc_idx -4, and upstream returned `(4, True, eeeeeeee)`. Generator:
    /// key = sha256(init[8:40] + secret), iv = init[40:56], init[56:64] =
    /// tail ^ ctr_keystream[56:64], with init = sha256(b"tgx-vector-a") +
    /// sha256(b"tgx-vector-b") and tail = ee*4 + pack('<h', -4) + 5a a5;
    /// then `_try_handshake(init, secret)` extracted from tg_ws_proxy.py.
    #[test]
    fn matches_upstream_vector() {
        const INIT: &str = "91209f57eaae6adbc23e576b3003d3c456ac1cafe1cddc49a44ee9144449160e\
                            1cc317c4beddd550fbf6b1e6f65fe37d56bbd69f6b47e7ca552e07f6e704699c";
        let bytes: Vec<u8> = (0..64)
            .map(|i| u8::from_str_radix(&INIT[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let init: [u8; 64] = bytes.try_into().unwrap();
        let secret: [u8; 16] = core::array::from_fn(|i| i as u8);
        let got = parse_client_init(&init, &secret).unwrap();
        assert_eq!(
            (got.dc, got.media, got.proto),
            (4, true, ProtoTag::Intermediate)
        );
    }
}
