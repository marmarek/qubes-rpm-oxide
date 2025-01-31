//! Utility functions for parsing OpenPGP packets

use super::{Error, Reader};
#[cfg(feature = "alloc")]
extern crate alloc;

/// The format of a packet
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum Format {
    /// Old format
    Old = 0,
    /// New format
    New = 0x40,
}

/// An OpenPGP packet
#[derive(Clone, Debug)]
pub struct Packet<'a> {
    tag: u8,
    buffer: &'a [u8],
}

pub(crate) fn get_varlen_bytes<'a>(reader: &mut Reader<'a>) -> Result<&'a [u8], Error> {
    let keybyte: u8 = reader.byte()?;
    let len: usize = match keybyte {
        0...191 => keybyte.into(),
        192...223 => ((usize::from(keybyte) - 192) << 8) + usize::from(reader.byte()?) + 192,
        255 => reader.be_u32()? as _,
        // Partial lengths are deliberately unsupported, as we don’t handle PGP signed and/or
        // encrypted data ourselves.
        _ => return Err(Error::PartialLength),
    };
    Ok(reader.get_bytes(len)?)
}

/// Read a packet from `reader`.  Returns:
///
/// - `Ok(Some(packet))` if a packet is read
/// - `Ok(None)` if the reader is empty.
/// - `Err` if an error occurred, such as trailing junk.
pub fn next<'a>(reader: &mut Reader<'a>) -> Result<Option<Packet<'a>>, Error> {
    let tagbyte: u8 = match reader.maybe_byte() {
        Some(e) if e & 0x80 == 0 => return Err(Error::PacketFirstBitZero),
        Some(e) => e,
        None => return Ok(None),
    };
    let packet = if tagbyte & 0x40 == 0 {
        let lenlen = 1u8 << (tagbyte & 0b11);
        // We deliberately do not support indefinite-length packets.
        if lenlen > 4 {
            return Err(Error::PartialLength);
        }
        let mut len = 0usize;
        for &i in reader.get_bytes(usize::from(lenlen))? {
            len = len << 8 | usize::from(i)
        }
        Packet {
            tag: 0xF & (tagbyte >> 2),
            buffer: reader.get_bytes(len)?,
        }
    } else {
        let buffer = get_varlen_bytes(reader)?;
        Packet {
            tag: tagbyte & 0x3F,
            buffer,
        }
    };
    if packet.tag != 0 {
        Ok(Some(packet))
    } else {
        Err(Error::BadTag)
    }
}

impl<'a> Packet<'a> {
    /// Retrieves the packet’s tag.  Will always return non-zero.
    pub fn tag(&self) -> u8 {
        self.tag & 0x3F
    }

    /// Retrieves the packet’s contents as a slice.
    pub fn contents(&self) -> &'a [u8] {
        self.buffer
    }

    /// Wraps the packet in OpenPGP encapsulation
    #[cfg(feature = "alloc")]
    pub fn serialize(&self) -> alloc::vec::Vec<u8> {
        let len = self.buffer.len();
        assert!(u64::from(u32::max_value()) >= len as u64);
        let tag_byte = self.tag | 0b1100_0000u8;
        let mut v = match len {
            0...191 => {
                // 1-byte
                let mut v = alloc::vec::Vec::with_capacity(2 + len);
                v.push(tag_byte);
                v.push(len as u8);
                v
            }
            192...8383 => {
                // 2-byte
                let mut v = alloc::vec::Vec::with_capacity(3 + len);
                let len = len - 192;
                v.push(tag_byte);
                v.push((len >> 8) as u8 + 192);
                v.push(len as u8);
                v
            }
            _ => {
                // 5-byte
                let mut v = alloc::vec::Vec::with_capacity(6 + len);
                v.extend_from_slice(&[
                    tag_byte,
                    0xFF,
                    (len >> 24) as u8,
                    (len >> 16) as u8,
                    (len >> 8) as u8,
                    len as u8,
                ]);
                v
            }
        };
        v.extend_from_slice(self.buffer);
        v
    }
}

#[cfg(all(feature = "alloc", test))]
mod tests {
    use super::*;
    fn serialize(tag: u8, buffer: &[u8]) -> alloc::vec::Vec<u8> {
        Packet { tag, buffer }.serialize()
    }
    #[test]
    fn check_packet_serialization_short() {
        assert_eq!(serialize(0x4F, &[][..]), vec![0b1100_1111, 0x0]);
        assert_eq!(serialize(0x7, &[b'a'][..]), vec![0b1100_0111, 0x1, b'a']);
        assert_eq!(serialize(0x10, &[b'a'][..]), vec![0b1101_0000, 0x1, b'a']);
    }

    /// Create an old-format packet
    #[test]
    fn old_format_parsing() {
        let mut buffer = vec![0u8; (1usize << 28) + 5];
        for tag in 1..16 {
            buffer[0] = 0x81 | tag << 2;
            buffer[1] = 0;
            {
                let mut reader = Reader::new(&buffer[..1]);
                assert_eq!(next(&mut reader).unwrap_err(), Error::PrematureEOF);
            }
            for len in 0..256 {
                buffer[0] = 0x80 | tag << 2;
                buffer[1] = len as _;
                let mut reader = Reader::new(&buffer[..len + 2]);
                let mut packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 0);
                reader = Reader::new(&buffer[..len + 1]);
                assert_eq!(next(&mut reader).unwrap_err(), Error::PrematureEOF);
                reader = Reader::new(&buffer[..len + 3]);
                packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 1);
            }
            buffer[0] = 0x81 | tag << 2;
            buffer[1] = 0;
            buffer[2] = 0;
            for i in 1..3 {
                let mut reader = Reader::new(&buffer[..i]);
                assert_eq!(next(&mut reader).unwrap_err(), Error::PrematureEOF);
            }
            for len in 0..65536 {
                buffer[1] = (len >> 8) as _;
                buffer[2] = len as _;
                let mut reader = Reader::new(&buffer[..len + 3]);
                let mut packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 0);
                reader = Reader::new(&buffer[..len + 2]);
                assert_eq!(next(&mut reader).unwrap_err(), Error::PrematureEOF);
                reader = Reader::new(&buffer[..len + 4]);
                packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 1);
            }
            for len in 0..0x100000 {
                // we cannot test every value, so we test a subset instead
                let len = len * 100 + 10;
                buffer[0] = 0x82 | tag << 2;
                buffer[1] = (len >> 24) as _;
                buffer[2] = (len >> 16) as _;
                buffer[3] = (len >> 8) as _;
                buffer[4] = len as _;
                let mut reader = Reader::new(&buffer[..len + 5]);
                let packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 0);
                reader = Reader::new(&buffer[..len + 4]);
                assert_eq!(next(&mut reader).unwrap_err(), Error::PrematureEOF);
                reader = Reader::new(&buffer[..len + 6]);
                let packet = next(&mut reader).unwrap().unwrap();
                assert_eq!(packet.buffer.len(), len);
                assert_eq!(packet.tag, tag);
                assert_eq!(reader.len(), 1);
            }
            buffer[0] = 0x83 | tag << 2;
            let mut reader = Reader::new(&buffer[..20]);
            next(&mut reader).unwrap_err();
        }
    }
    #[test]
    fn check_packet_serialization() {
        assert_eq!(0b1100_0000, 0xC0);
        let buffer = vec![0u8; 65536];
        for tag in 1..64 {
            for j in 0..buffer.len() {
                let serialized = Packet {
                    tag,
                    buffer: &buffer[..j],
                }
                .serialize();
                assert_eq!(serialized[0] & 0b1100_0000, 0b1100_0000);
                assert_eq!(serialized[0] & 0b0011_1111, tag);
                if j < 192 {
                    assert_eq!(usize::from(serialized[1]), j);
                    assert_eq!(serialized.len(), j + 2);
                } else if j < 8384 {
                    assert_eq!(serialized.len(), j + 3);
                    let (fst, snd) = (serialized[1], serialized[2]);
                    assert!(fst >= 192 && fst < 224);
                    assert_eq!((usize::from(fst) - 192) << 8 | usize::from(snd), j - 192);
                    if j == 8383 {
                        assert_eq!(fst, 223);
                        assert_eq!(snd, 255);
                    } else if j == 192 {
                        assert_eq!(fst, 192);
                        assert_eq!(snd, 0);
                    }
                    for k in 1..6 {
                        let mut short_reader = Reader::new(&serialized[..k]);
                        assert_eq!(next(&mut short_reader).unwrap_err(), Error::PrematureEOF);
                    }
                } else {
                    assert_eq!(serialized.len(), j + 6);
                    assert_eq!(serialized[1], 255);
                    assert_eq!(
                        (serialized[2] as u32) << 24
                            | (serialized[3] as u32) << 16
                            | (serialized[4] as u32) << 8
                            | (serialized[5] as u32),
                        j as u32
                    );
                    for k in 1..6 {
                        let mut short_reader = Reader::new(&serialized[..k]);
                        assert_eq!(next(&mut short_reader).unwrap_err(), Error::PrematureEOF);
                    }
                }
                {
                    let mut short_reader = Reader::new(&serialized[..serialized.len() - 1]);
                    assert_eq!(next(&mut short_reader).unwrap_err(), Error::PrematureEOF);
                }
                let mut reader = Reader::new(&serialized);
                let Packet {
                    tag: deserialized_tag,
                    buffer: deserialized_buffer,
                } = next(&mut reader).unwrap().unwrap();
                assert_eq!(reader.len(), 0);
                assert_eq!(tag, deserialized_tag);
                assert_eq!(&buffer[..j], deserialized_buffer);
            }
        }
    }
}
