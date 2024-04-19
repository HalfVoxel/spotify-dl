use std::{io::Write, path::Path};
use mp3lame_encoder::{Builder, Id3Tag, DualPcm, FlushNoGap};

use audiotags::{Tag, TagType};
use librespot::playback::{
    audio_backend::{Open, Sink, SinkError},
    config::AudioFormat,
    convert::Converter,
    decoder::AudioPacket,
};

use crate::TrackMetadata;

pub struct FileSinkMP3 {
    sink: String,
    content: Vec<i16>,
    metadata: Option<TrackMetadata>,
    compression: u32,
}

impl FileSinkMP3 {
    pub fn add_metadata(&mut self, meta: TrackMetadata) {
        self.metadata = Some(meta);
    }
    pub fn set_compression(&mut self, compression: u32) {
        self.compression = compression;
    }
}

impl Open for FileSinkMP3 {
    fn open(path: Option<String>, _audio_format: AudioFormat) -> Self {
        let file_path = path.unwrap_or_else(|| panic!());
        FileSinkMP3 {
            sink: file_path,
            content: Vec::new(),
            metadata: None,
            compression: 4,
        }
    }
}

impl Sink for FileSinkMP3 {
    fn start(&mut self) -> Result<(), SinkError> {
        Ok(())
    }

    fn stop(&mut self) -> Result<(), SinkError> {
        let mut mp3_encoder = Builder::new().expect("Create LAME builder");
        mp3_encoder.set_num_channels(2).expect("set channels");
        mp3_encoder.set_sample_rate(44_100).expect("set sample rate");
        mp3_encoder.set_brate(mp3lame_encoder::Bitrate::Kbps192).expect("set brate");
        mp3_encoder.set_quality(mp3lame_encoder::Quality::Best).expect("set quality");
        match &self.metadata {
            Some(meta) => {
                mp3_encoder.set_id3_tag(Id3Tag {
                    title: meta.track_name.as_bytes(),
                    artist: meta.artists.join(", ").as_bytes(),
                    album: meta.album.as_bytes(),
                    year: b"",
                    comment: b"",
                });
            }
            None => (),
        }
        let mut mp3_encoder = mp3_encoder.build().expect("To initialize LAME encoder");

        // Content is interleaved, convert it to separate channels
        let left = self.content.iter().step_by(2).copied().collect::<Vec<_>>();
        let right = self.content.iter().skip(1).step_by(2).copied().collect::<Vec<_>>();
        //use actual PCM data
        let input = DualPcm {
            left: &left,
            right: &right,
        };

        let mut mp3_out_buffer = Vec::new();
        mp3_out_buffer.reserve(mp3lame_encoder::max_required_buffer_size(input.left.len()));
        let encoded_size = mp3_encoder.encode(input, mp3_out_buffer.spare_capacity_mut()).expect("To encode");
        unsafe {
            mp3_out_buffer.set_len(mp3_out_buffer.len().wrapping_add(encoded_size));
        }

        let encoded_size = mp3_encoder.flush::<FlushNoGap>(mp3_out_buffer.spare_capacity_mut()).expect("to flush");
        unsafe {
            mp3_out_buffer.set_len(mp3_out_buffer.len().wrapping_add(encoded_size));
        }

        if let Err(e) = atomicwrites::AtomicFile::new(&self.sink, atomicwrites::OverwriteBehavior::AllowOverwrite)
            .write(|f| f.write(&mp3_out_buffer)) {
                return Err(SinkError::OnWrite(e.to_string()));
            }
        
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> Result<(), SinkError> {
        let mut data = converter.f64_to_s16(packet.samples().unwrap());
        self.content.append(&mut data);
        Ok(())
    }
}
