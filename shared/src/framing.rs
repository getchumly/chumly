//! Message framing on a stream: 4-byte length (little endian),
//! followed by a protobuf-encoded body (see `wire.rs`).

use anyhow::bail;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::wire::Wire;

pub const MAX_FRAME_BYTES: usize = 1 << 20;

pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Wire,
{
    let body = msg.encode_wire();
    if body.len() > MAX_FRAME_BYTES {
        bail!("frame too large: {} bytes", body.len());
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await?;
    Ok(())
}

/// Returns `None` if the stream was closed cleanly between frames.
/// Messages of a type this version does not know (from a newer peer) are
/// skipped.
pub async fn read_msg<R, T>(r: &mut R) -> anyhow::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: Wire,
{
    read_msg_limited(r, MAX_FRAME_BYTES).await
}

/// Like [`read_msg`], but rejects frames larger than `max` bytes. Used for
/// messages from unauthenticated peers, which must not make us allocate much.
pub async fn read_msg_limited<R, T>(r: &mut R, max: usize) -> anyhow::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: Wire,
{
    loop {
        let mut len_buf = [0u8; 4];
        match r.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > max {
            bail!("frame too large: {len} bytes");
        }
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).await?;
        if let Some(msg) = T::decode_wire(&body)? {
            return Ok(Some(msg));
        }
        // Unknown message type: skip the frame and read the next one.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ClientMsg;

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let msg = ClientMsg::ChatSend { channel_id: 3, text: "hello".into() };
        write_msg(&mut a, &msg).await.unwrap();
        drop(a);
        let got: Option<ClientMsg> = read_msg(&mut b).await.unwrap();
        assert_eq!(got, Some(msg));
        let end: Option<ClientMsg> = read_msg(&mut b).await.unwrap();
        assert_eq!(end, None);
    }

    #[tokio::test]
    async fn skips_unknown_message_types() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        // A frame with a message type from the future (tag 99), then a known one.
        let unknown = [0xfa, 0x06, 0x00]; // field 99, length-delimited, empty
        let mut frame = (unknown.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&unknown);
        a.write_all(&frame).await.unwrap();
        let msg = ClientMsg::ListUsers;
        write_msg(&mut a, &msg).await.unwrap();
        let got: Option<ClientMsg> = read_msg(&mut b).await.unwrap();
        assert_eq!(got, Some(msg));
    }

    #[tokio::test]
    async fn rejects_garbage() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        a.write_all(&[4, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]).await.unwrap();
        assert!(read_msg::<_, ClientMsg>(&mut b).await.is_err());
    }

    #[tokio::test]
    async fn limited_rejects_large_frame() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let msg = ClientMsg::ChatSend { channel_id: 3, text: "x".repeat(100) };
        write_msg(&mut a, &msg).await.unwrap();
        assert!(read_msg_limited::<_, ClientMsg>(&mut b, 16).await.is_err());
    }
}
