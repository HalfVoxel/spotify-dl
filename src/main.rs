mod file_sink_flac;
mod file_sink_mp3;

extern crate rpassword;

use librespot::core::cache::Cache;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session;
use librespot::core::spotify_id::SpotifyId;
use librespot::playback::config::PlayerConfig;
use librespot::playback::mixer::NoOpVolume;
use librespot::{core::authentication::Credentials, metadata::Playlist};
use std::path::Path;
use std::path::PathBuf;

use librespot::playback::audio_backend::Open;
use librespot::playback::player::Player;

use librespot::metadata::{Album, Artist, Metadata, Track};

use regex::Regex;
use structopt::StructOpt;

use indicatif::{ProgressBar, ProgressStyle};

#[derive(Debug, StructOpt)]
#[structopt(
    name = "spotify-dl",
    about = "A commandline utility to download music directly from Spotify"
)]
struct Opt {
    #[structopt(help = "A list of Spotify URIs (songs, podcasts or playlists)")]
    tracks: Vec<String>,
    #[structopt(
        short = "c",
        long = "credentials-dir",
        help = "Your Spotify credentials cache directory"
    )]
    credentials_cache: String,
    #[structopt(
        short = "d",
        long = "destination",
        default_value = ".",
        help = "The directory where the songs will be downloaded"
    )]
    destination: String,
    #[structopt(
        short = "o",
        long = "ordered",
        help = "Prefixing the filename with its index in the playlist"
    )]
    ordered: bool,
    #[structopt(
        short = "r",
        long = "delete-unknown-songs",
        help = "Delete songs from the destination that are not in the playlist."
    )]
    delete_unknown_songs: bool,
}

#[derive(Clone)]
pub struct TrackMetadata {
    artists: Vec<String>,
    track_name: String,
    album: String,
}

async fn create_session(
    session_config: SessionConfig,
    credentials: Credentials,
    cache: Option<Cache>,
) -> Session {
    let (s, _) = Session::connect(session_config, credentials, cache, false)
        .await
        .expect("Failed to connect to Spotify");
    s
}

fn make_filename_compatible(filename: &str) -> String {
    let invalid_chars = ['<', '>', ':', '\'', '"', '/', '\\', '|', '?', '*'];
    let mut clean = String::new();
    for c in filename.chars() {
        if !invalid_chars.contains(&c) && c.is_ascii() && !c.is_control() && c.len_utf8() == 1 {
            clean.push(c);
        }
    }
    clean
}

#[derive(Clone, Copy, Debug)]
pub enum Encoding {
    Flac { compression: Option<u32> },
    Mp3,
}

fn extension_from_encoding(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Flac { .. } => "flac",
        Encoding::Mp3 => "mp3",
    }
}

async fn download_tracks(
    session: &Session,
    destination: PathBuf,
    tracks: Vec<SpotifyId>,
    ordered: bool,
    encoding: Encoding,
    delete_unknown_songs: bool,
) {
    let player_config = PlayerConfig::default();
    let bar_style = ProgressStyle::default_bar()
        .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} (ETA: {eta}) {msg}")
        .progress_chars("##-");
    let bar = ProgressBar::new(tracks.len() as u64);
    bar.set_style(bar_style);
    bar.enable_steady_tick(500);

    let mut all_files: Vec<String> = vec![];
    for (i, track) in tracks.iter().enumerate() {
        let track_item = Track::get(&session, *track).await.unwrap();
        let artist_name: String;

        let mut metadata = TrackMetadata {
            artists: Vec::new(),
            track_name: track_item.name,
            album: Album::get(session, track_item.album).await.unwrap().name,
        };
        if track_item.artists.len() > 1 {
            let mut tmp: String = String::new();
            for artist in track_item.artists {
                let artist_item = Artist::get(&session, artist).await.unwrap();
                metadata.artists.push(artist_item.name.clone());
                tmp.push_str(artist_item.name.as_str());
                tmp.push_str(", ");
            }
            artist_name = String::from(tmp.trim_end_matches(", "));
        } else {
            artist_name = Artist::get(&session, track_item.artists[0])
                .await
                .unwrap()
                .name;
            metadata.artists.push(artist_name.clone());
        }

        let full_track_name = format!("{} - {}", artist_name, metadata.track_name);
        let full_track_name_clean = make_filename_compatible(full_track_name.as_str());
        //let filename = format!("{}.flac", full_track_name_clean);
        let filename: String;
        let extension = extension_from_encoding(encoding);
        if ordered {
            filename = format!("{:03} - {full_track_name_clean}.{extension}", i + 1);
        } else {
            filename = format!("{full_track_name_clean}.{extension}");
        }
        let joined_path = destination.join(&filename);
        let path = joined_path.to_str().unwrap();
        bar.set_message(full_track_name_clean.as_str());

        let file_name = Path::new(path).file_stem().unwrap().to_str().unwrap();
        all_files.push(file_name.to_string());

        let path_parent = Path::new(path).parent().unwrap();
        let entries = path_parent.read_dir().unwrap();

        // Check if a file with the same name (but not necessarily the same extension) already exists
        let mut file_exists = false;
        for entry in entries {
            let entry = entry.unwrap();
            let entry_path = entry.path();
            let entry_file_name = entry_path.file_stem().unwrap().to_str().unwrap();
            if entry_file_name == file_name {
                file_exists = true;
                break;
            }
        }

        if !file_exists {
            let mut file_sink = file_sink_mp3::FileSinkMP3::open( // file_sink_flac::FileSinkFlac::open(
                Some(path.to_owned()),
                librespot::playback::config::AudioFormat::S16,
            );
            file_sink.add_metadata(metadata);
            match &encoding {
                Encoding::Flac { compression } => file_sink.set_compression(compression.unwrap_or(4)),
                _ => {}
            }
            let (mut player, _) =
                Player::new(player_config.clone(), session.clone(), Box::new(NoOpVolume), move || {
                    Box::new(file_sink)
                });
            player.load(*track, true, 0);
            player.await_end_of_track().await;
            bar.set_message(&format!("{full_track_name_clean} - Encoding..."));
            player.stop();
            bar.inc(1);
        } else {
            // println!("File with the same name already exists, skipping: {}", path);
            bar.inc(1);
        }
    }

    if delete_unknown_songs {
        for entry in destination.read_dir().unwrap() {
            let entry_path = entry.unwrap().path();
            if !match entry_path.extension().map(|s| s.to_str().unwrap()) {
                Some("mp3") | Some("flac") | Some("wav") | Some("ogg") => true,
                Some(_) => false,
                None => false,
            } {
                continue;
            }

            let file_name = entry_path.file_stem().unwrap().to_str().unwrap();
            if !all_files.iter().any(|s| s == file_name) {
                println!("Deleting existing file which is not part of the playlist: {file_name}");
                std::fs::remove_file(entry_path).unwrap();
            }
        }
    }
    bar.finish();
}

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();

    let session_config = SessionConfig::default();
    let cache = Cache::new(Some(opt.credentials_cache), None, None, None).unwrap();
    let session = create_session(session_config, cache.credentials().unwrap(), Some(cache)).await;

    let mut tracks: Vec<SpotifyId> = Vec::new();

    for track_url in opt.tracks {
        let track = SpotifyId::from_uri(track_url.as_str()).unwrap_or_else(|_| {
            let regex = Regex::new(r"https://open.spotify.com/(\w+)/(.*)\?").unwrap();

            let results = regex.captures(track_url.as_str()).unwrap();
            let uri = format!(
                "spotify:{}:{}",
                results.get(1).unwrap().as_str(),
                results.get(2).unwrap().as_str()
            );

            SpotifyId::from_uri(&uri).unwrap()
        });
        match &track.audio_type {
            librespot::core::spotify_id::SpotifyAudioType::Track => {
                tracks.push(track);
            }
            librespot::core::spotify_id::SpotifyAudioType::Podcast => {
                tracks.push(track);
            }
            librespot::core::spotify_id::SpotifyAudioType::NonPlayable => {
                match Playlist::get(&session, track).await {
                    Ok(mut playlist) => {
                        println!(
                            "Adding all songs from playlist {} (by {}) to the queue",
                            &playlist.name, &playlist.user
                        );
                        tracks.append(&mut playlist.tracks);
                    }
                    Err(_) => {
                        println!("Unsupported track {}", &track_url);
                    }
                }
            }
        }
    }

    download_tracks(
        &session,
        PathBuf::from(opt.destination),
        tracks,
        opt.ordered,
        Encoding::Mp3,
        // opt.compression,
        opt.delete_unknown_songs,
    )
    .await;
}
