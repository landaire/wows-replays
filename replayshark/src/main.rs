use anyhow::{anyhow, Context};
use clap::{App, Arg, SubCommand};
use std::borrow::Cow;
use std::fs::read_dir;
use std::io::{Cursor, Write};
use std::{collections::HashMap, path::Path};
use wowsunpack::data::idx;
use wowsunpack::data::pkg::PkgFileLoader;
use wowsunpack::rpc::entitydefs::EntitySpec;
use wowsunpack::{
    data::{DataFileLoader, DataFileWithCallback, Version},
    rpc::entitydefs::parse_scripts,
};

use wows_replays::{
    analyzer::{
        chat::ChatLoggerBuilder, summary::SummaryBuilder, AnalyzerAdapter, AnalyzerBuilder,
        AnalyzerMutBuilder,
    },
    ErrorKind, ReplayFile,
};

struct InvestigativePrinter {
    filter_packet: Option<u32>,
    filter_method: Option<String>,
    timestamp: Option<f32>,
    entity_id: Option<u32>,
    meta: bool,
    version: Version,
}

impl wows_replays::analyzer::AnalyzerMut for InvestigativePrinter {
    fn finish(&mut self) {}

    fn process_mut(&mut self, packet: &wows_replays::packet2::Packet<'_, '_>) {
        let decoded =
            wows_replays::analyzer::decoder::DecodedPacket::from(&self.version, true, packet);

        if self.meta {
            match &decoded.payload {
                wows_replays::analyzer::decoder::DecodedPacketPayload::OnArenaStateReceived {
                    players,
                    ..
                } => {
                    for player in players.iter() {
                        println!(
                            "{} {}/{} ({:x?}/{:x?})",
                            player.username,
                            player.meta_ship_id,
                            player.avatar_id,
                            (player.meta_ship_id as u32).to_le_bytes(),
                            (player.avatar_id as u32).to_le_bytes()
                        );
                    }
                }
                _ => {
                    // Nop
                }
            }
        }

        if let Some(n) = self.filter_packet {
            if n != decoded.packet_type {
                return;
            }
        }
        if let Some(s) = self.filter_method.as_ref() {
            match &packet.payload {
                wows_replays::packet2::PacketType::EntityMethod(method) => {
                    if method.method != s {
                        return;
                    }
                    if let Some(eid) = self.entity_id {
                        if method.entity_id != eid {
                            return;
                        }
                    }
                }
                _ => {
                    return;
                }
            }
        }
        if let Some(t) = self.timestamp {
            let clock = (decoded.clock + t) as u32;
            let s = clock % 60;
            let clock = (clock - s) / 60;
            let m = clock % 60;
            let clock = (clock - m) / 60;
            let h = clock;
            let encoded = if self.filter_method.is_some() {
                match &packet.payload {
                    wows_replays::packet2::PacketType::EntityMethod(method) => {
                        serde_json::to_string(&method).unwrap()
                    }
                    _ => panic!(),
                }
            } else if self.filter_packet.is_some() {
                match &packet.payload {
                    wows_replays::packet2::PacketType::Unknown(x) => {
                        let v: Vec<_> = x.iter().map(|n| format!("{:02x}", n)).collect();
                        format!("0x[{}]", v.join(","))
                    }
                    _ => serde_json::to_string(&packet).unwrap(),
                }
            } else {
                serde_json::to_string(&decoded).unwrap()
            };
            println!("{:02}:{:02}:{:02}: {}", h, m, s, encoded);
        } else {
            let encoded = serde_json::to_string(&decoded).unwrap();
            println!("{}", &encoded);
        }
    }
}

pub struct InvestigativeBuilder {
    no_meta: bool,
    filter_packet: Option<String>,
    filter_method: Option<String>,
    timestamp: Option<String>,
    entity_id: Option<String>,
}

impl AnalyzerMutBuilder for InvestigativeBuilder {
    fn build(
        &self,
        meta: &wows_replays::ReplayMeta,
    ) -> Box<dyn wows_replays::analyzer::AnalyzerMut> {
        let version = Version::from_client_exe(&meta.clientVersionFromExe);
        let decoder = InvestigativePrinter {
            version: version,
            filter_packet: self
                .filter_packet
                .as_ref()
                .map(|s| parse_int::parse::<u32>(s).unwrap()),
            filter_method: self.filter_method.clone(),
            timestamp: self.timestamp.as_ref().map(|s| {
                let ts_parts: Vec<_> = s.split("+").collect();
                let offset = ts_parts[1].parse::<u32>().unwrap();
                let parts: Vec<_> = ts_parts[0].split(":").collect();
                if parts.len() == 3 {
                    let h = parts[0].parse::<u32>().unwrap();
                    let m = parts[1].parse::<u32>().unwrap();
                    let s = parts[2].parse::<u32>().unwrap();
                    (h * 3600 + m * 60 + s) as f32 - offset as f32
                } else {
                    panic!("Expected hh:mm:ss+offset as timestamp");
                }
            }),
            entity_id: self
                .entity_id
                .as_ref()
                .map(|s| parse_int::parse(s).unwrap()),
            meta: !self.no_meta,
        };
        if !self.no_meta {
            println!("{}", &serde_json::to_string(&meta).unwrap());
        }
        Box::new(decoder)
    }
}

fn load_game_data(
    game_dir: Option<&str>,
    extracted_dir: Option<&str>,
    replay_version: &Version,
) -> anyhow::Result<Vec<EntitySpec>> {
    let specs = match (game_dir, extracted_dir) {
        (Some(game_dir), _) => {
            let mut idx_files = Vec::new();
            let wows_directory = Path::new(game_dir);

            let mut latest_build = None;
            for file in read_dir(wows_directory.join("bin"))? {
                if file.is_err() {
                    continue;
                }

                let file = file.unwrap();
                if let Ok(ty) = file.file_type() {
                    if ty.is_file() {
                        continue;
                    }

                    if let Some(build_num) = file
                        .file_name()
                        .to_str()
                        .and_then(|name| name.parse::<usize>().ok())
                    {
                        if latest_build.is_none()
                            || latest_build
                                .map(|number| number < build_num)
                                .unwrap_or(false)
                        {
                            latest_build = Some(build_num)
                        }
                    }
                }
            }

            if latest_build.is_none() {
                return Err(anyhow!(
                    "Could not determine latest WoWs build from the provided game directory"
                ));
            }

            for file in read_dir(
                wows_directory
                    .join("bin")
                    .join(latest_build.unwrap().to_string())
                    .join("idx"),
            )
            .context("failed to read wows idx directory")?
            {
                let file = file.unwrap();
                if file.file_type().unwrap().is_file() {
                    let file_data = std::fs::read(file.path()).unwrap();
                    let mut file = Cursor::new(file_data.as_slice());
                    idx_files.push(idx::parse(&mut file).unwrap());
                }
            }

            let pkgs_path = wows_directory.join("res_packages");
            if !pkgs_path.exists() {
                return Err(anyhow!("Invalid wows directory -- res_packages not found"));
            }

            let pkg_loader = PkgFileLoader::new(pkgs_path);

            let file_tree = idx::build_file_tree(idx_files.as_slice());

            let loader = DataFileWithCallback::new(|path| {
                let path = Path::new(path);

                let mut file_data = Vec::new();
                file_tree
                    .read_file_at_path(path, &pkg_loader, &mut file_data)
                    .with_context(|| {
                        format!("failed to read file from packed game files: {:?}", path)
                    })
                    .unwrap();

                Ok(Cow::Owned(file_data))
            });

            parse_scripts(&loader).unwrap()
        }
        (None, Some(extracted)) => {
            let extracted_dir = Path::new(extracted).join(replay_version.to_path());
            if !extracted_dir.exists() {
                return Err(anyhow!(
                    "Missing scripts for game version {}. Expected to be at {:?}",
                    replay_version.to_path(),
                    &extracted_dir
                ));
            }
            let loader = DataFileWithCallback::new(|path| {
                let path = Path::new(path);

                let file_data = std::fs::read(extracted_dir.join(path))
                    .with_context(|| {
                        format!("failed to read game file from extracted dir: {:?}", path)
                    })
                    .unwrap();

                Ok(Cow::Owned(file_data))
            });
            parse_scripts(&loader).unwrap()
        }
        (None, None) => {
            return Err(anyhow!(
                "Game directory or extracted files directory must be supplied"
            ));
        }
    };

    Ok(specs)
}

fn parse_replay<P: wows_replays::analyzer::AnalyzerMutBuilder>(
    replay: &std::path::PathBuf,
    game_dir: Option<&str>,
    extracted_dir: Option<&str>,
    processor: P,
) -> Result<(), wows_replays::ErrorKind> {
    let replay_file = ReplayFile::from_file(replay)?;

    //let mut file = std::fs::File::create("foo.bin").unwrap();
    //file.write_all(&replay_file.packet_data).unwrap();

    let specs = load_game_data(
        game_dir,
        extracted_dir,
        &Version::from_client_exe(replay_file.meta.clientVersionFromExe.as_str()),
    )
    .expect("failed to load game specs");

    let version_parts: Vec<_> = replay_file.meta.clientVersionFromExe.split(",").collect();
    assert!(version_parts.len() == 4);

    let processor = processor.build(&replay_file.meta);

    // Parse packets
    let mut p = wows_replays::packet2::Parser::new(&specs);
    let mut analyzer_set = AnalyzerAdapter::new(vec![processor]);
    match p.parse_packets_mut::<AnalyzerAdapter>(&replay_file.packet_data, &mut analyzer_set) {
        Ok(()) => {
            analyzer_set.finish();
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn truncate_string(s: &str, length: usize) -> &str {
    match s.char_indices().nth(length) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

fn printspecs(specs: &Vec<wowsunpack::rpc::entitydefs::EntitySpec>) {
    println!("Have {} entities", specs.len());
    for entity in specs.iter() {
        println!();
        println!(
            "{} has {} properties ({} internal) and {}/{}/{} base/cell/client methods",
            entity.name,
            entity.properties.len(),
            entity.internal_properties.len(),
            entity.base_methods.len(),
            entity.cell_methods.len(),
            entity.client_methods.len()
        );

        println!("Properties:");
        for (i, property) in entity.properties.iter().enumerate() {
            println!(
                " - {}: {} flag={:?} type={:?}",
                i, property.name, property.flags, property.prop_type
            );
        }
        println!("Internal properties:");
        for (i, property) in entity.internal_properties.iter().enumerate() {
            println!(" - {}: {} type={:?}", i, property.name, property.prop_type);
        }
        println!("Client methods:");
        for (i, method) in entity.client_methods.iter().enumerate() {
            println!(" - {}: {}", i, method.name);
            for arg in method.args.iter() {
                println!("      - {:?}", arg);
            }
        }
    }
}

enum SurveyResult {
    /// npackets, ninvalid
    Success((String, String, usize, usize, Vec<String>)),
    UnsupportedVersion(String),
    ParseFailure(String),
}

struct SurveyResults {
    version_failures: usize,
    parse_failures: usize,
    successes: usize,
    successes_with_invalids: usize,
    total: usize,
    invalid_versions: HashMap<String, usize>,
    audits: HashMap<String, (String, Vec<String>)>,
}

impl SurveyResults {
    fn empty() -> Self {
        Self {
            version_failures: 0,
            parse_failures: 0,
            successes: 0,
            successes_with_invalids: 0,
            total: 0,
            invalid_versions: HashMap::new(),
            audits: HashMap::new(),
        }
    }

    fn add(&mut self, result: SurveyResult) {
        self.total += 1;
        match result {
            SurveyResult::Success((hash, datetime, _npacks, ninvalid, audits)) => {
                self.successes += 1;
                if ninvalid > 0 {
                    self.successes_with_invalids += 1;
                }
                if audits.len() > 0 {
                    self.audits.insert(hash, (datetime, audits));
                }
            }
            SurveyResult::UnsupportedVersion(version) => {
                self.version_failures += 1;
                if !self.invalid_versions.contains_key(&version) {
                    self.invalid_versions.insert(version.clone(), 0);
                }
                *self.invalid_versions.get_mut(&version).unwrap() += 1;
            }
            SurveyResult::ParseFailure(_error) => {
                self.parse_failures += 1;
            }
        }
    }

    fn print(&self) {
        let mut audits: Vec<_> = self.audits.iter().collect();
        audits.sort_by_key(|(_, (tm, _))| {
            chrono::NaiveDateTime::parse_from_str(tm, "%d.%m.%Y %H:%M:%S").unwrap()
        });
        for (k, (tm, v)) in audits.iter() {
            println!();
            println!(
                "{} ({}) has {} audits:",
                truncate_string(k, 20),
                tm,
                v.len()
            );
            let mut cnt = 0;
            for audit in v.iter() {
                if cnt >= 10 {
                    println!("...truncating");
                    break;
                }
                println!(" - {}", audit);
                cnt += 1;
            }
        }
        println!();
        println!("Found {} replay files", self.total);
        println!(
            "- {} ({:.0}%) were parsed",
            self.successes,
            100. * self.successes as f64 / self.total as f64
        );
        println!(
            "  - Of which {} ({:.0}%) contained invalid packets",
            self.successes_with_invalids,
            100. * self.successes_with_invalids as f64 / self.successes as f64
        );
        println!(
            "- {} ({:.0}%) had a parse error",
            self.parse_failures,
            100. * self.parse_failures as f64 / self.total as f64
        );
        println!(
            "- {} ({:.0}%) are an unrecognized version",
            self.version_failures,
            100. * self.version_failures as f64 / self.total as f64
        );
        if self.invalid_versions.len() > 0 {
            for (k, v) in self.invalid_versions.iter() {
                println!("  - Version {} appeared {} times", k, v);
            }
        }
    }
}

fn survey_file(
    skip_decode: bool,
    game_dir: Option<&str>,
    extracted_dir: Option<&str>,
    replay: std::path::PathBuf,
) -> SurveyResult {
    let filename = replay.file_name().unwrap().to_str().unwrap();
    let filename = filename.to_string();

    print!("Parsing {}: ", truncate_string(&filename, 20));
    std::io::stdout().flush().unwrap();

    let survey_stats = std::rc::Rc::new(std::cell::RefCell::new(
        wows_replays::analyzer::survey::SurveyStats::new(),
    ));
    let survey =
        wows_replays::analyzer::survey::SurveyBuilder::new(survey_stats.clone(), skip_decode);
    match parse_replay(
        &std::path::PathBuf::from(replay),
        game_dir,
        extracted_dir,
        survey,
    ) {
        Ok(_) => {
            let stats = survey_stats.borrow();
            if stats.invalid_packets > 0 {
                println!(
                    "OK ({} packets, {} invalid)",
                    stats.total_packets, stats.invalid_packets
                );
            } else {
                println!("OK ({} packets)", stats.total_packets);
            }
            SurveyResult::Success((
                filename.to_string(),
                stats.date_time.clone(),
                stats.total_packets,
                stats.invalid_packets,
                stats.audits.clone(),
            ))
        }
        Err(ErrorKind::DatafileNotFound { version, .. }) => {
            println!("Unsupported version {}", version.to_path());
            SurveyResult::UnsupportedVersion(version.to_path())
        }
        Err(ErrorKind::UnsupportedReplayVersion(n)) => {
            println!("Unsupported version {}", n);
            SurveyResult::UnsupportedVersion(n)
        }
        Err(e) => {
            println!("Parse error: {:?}", e);
            SurveyResult::ParseFailure(format!("{:?}", e))
        }
    }
}

fn main() {
    let replay_arg = Arg::with_name("REPLAY")
        .help("The replay file to use")
        .required(true)
        .index(1);
    let matches = App::new("World of Warships Replay Parser Utility")
        .author("Lane Kolbly <lane@rscheme.org>")
        .about("Parses & processes World of Warships replay files")
        .arg(Arg::with_name("GAME_DIRECTORY").help("Path to your game directory. Should be the base game directory like E:\\WoWs\\World_of_Warships\\").short("g").long("game").takes_value(true))
        .arg(Arg::with_name("EXTRACTED_FILES_DIRECTORY").help("Path to extracted game files").short("e").long("extracted").takes_value(true))
        .subcommand(
            SubCommand::with_name("survey")
                .about("Runs the parser against a directory of replays to validate the parser")
                .arg(
                    Arg::with_name("skip-decode")
                        .long("skip-decode")
                        .help("Don't run the decoder"),
                )
                .arg(
                    Arg::with_name("REPLAYS")
                        .help("The replay files to use")
                        .required(true)
                        .multiple(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("chat")
                .about("Print the chat log of the given game")
                .arg(replay_arg.clone()),
        )
        .subcommand(
            SubCommand::with_name("summary")
                .about("Generate summary statistics of the game")
                .arg(replay_arg.clone()),
        )
        .subcommand(
            SubCommand::with_name("dump")
                .about("Dump the packets to console")
                .arg(
                    Arg::with_name("output")
                        .long("output")
                        .short("o")
                        .help("Output filename to dump to")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("no-meta")
                        .long("no-meta")
                        .help("Don't output the metadata as first line"),
                )
                .arg(replay_arg.clone()),
        )
        .subcommand(
            SubCommand::with_name("spec")
                .about("Dump the scripts specifications to console")
                .arg(
                    Arg::with_name("version")
                        .help("Version to dump. Must be comma-delimited: major,minor,patch,build")
                        .takes_value(true)
                        .required(true),
                )
        )
        .subcommand(
            SubCommand::with_name("search")
                .about("Search a directory full of replays")
                .arg(
                    Arg::with_name("REPLAYS")
                        .help("The replay files to use")
                        .required(true)
                        .multiple(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("investigate")
                .about("Tools designed for reverse-engineering packets")
                .arg(
                    Arg::with_name("meta")
                        .long("meta")
                        .help("Don't output the metadata as first line"),
                )
                .arg(
                    Arg::with_name("timestamp")
                        .long("timestamp")
                        .takes_value(true)
                        .help("hh:mm:ss offset to render clock values with"),
                )
                .arg(
                    Arg::with_name("filter-packet")
                        .long("filter-packet")
                        .takes_value(true)
                        .help("If specified, only return packets of the given packet_type"),
                )
                .arg(
                    Arg::with_name("filter-method")
                        .long("filter-method")
                        .takes_value(true)
                        .help("If specified, only return method calls for the given method"),
                )
                .arg(
                    Arg::with_name("entity-id")
                        .long("entity-id")
                        .takes_value(true)
                        .help("Entity ID to apply to other filters if applicable"),
                )
                .arg(replay_arg.clone()),
        );

    #[cfg(feature = "graphics")]
    let matches = matches.subcommand(
        SubCommand::with_name("trace")
            .about("Renders an image showing the trails of ships over the course of the game")
            .arg(
                Arg::with_name("out")
                    .long("output")
                    .help("Output PNG file to write")
                    .takes_value(true)
                    .required(true),
            )
            .arg(replay_arg.clone()),
    );

    let matches = matches.get_matches();

    let (game_dir, extracted) = (
        matches.value_of("GAME_DIRECTORY"),
        matches.value_of("EXTRACTED_FILES_DIRECTORY"),
    );

    if let Some(matches) = matches.subcommand_matches("dump") {
        let input = matches.value_of("REPLAY").unwrap();
        let dump = wows_replays::analyzer::decoder::DecoderBuilder::new(
            false,
            matches.is_present("no-meta"),
            matches.value_of("output"),
        );
        parse_replay(&std::path::PathBuf::from(input), game_dir, extracted, dump).unwrap();
    }
    if let Some(matches) = matches.subcommand_matches("investigate") {
        let input = matches.value_of("REPLAY").unwrap();
        let dump = InvestigativeBuilder {
            no_meta: !matches.is_present("meta"),
            filter_packet: matches.value_of("filter-packet").map(|s| s.to_string()),
            filter_method: matches.value_of("filter-method").map(|s| s.to_string()),
            entity_id: matches.value_of("entity-id").map(|s| s.to_string()),
            timestamp: matches.value_of("timestamp").map(|s| s.to_string()),
        };
        parse_replay(&std::path::PathBuf::from(input), game_dir, extracted, dump).unwrap();
    }
    if let Some(matches) = matches.subcommand_matches("spec") {
        let target_version = Version::from_client_exe(matches.value_of("version").unwrap());
        let specs =
            load_game_data(None, extracted, &target_version).expect("failed to load game data");
        printspecs(&specs);
    }
    if let Some(matches) = matches.subcommand_matches("summary") {
        let input = matches.value_of("REPLAY").unwrap();
        let dump = SummaryBuilder::new();
        parse_replay(&std::path::PathBuf::from(input), game_dir, extracted, dump).unwrap();
    }
    if let Some(matches) = matches.subcommand_matches("chat") {
        let input = matches.value_of("REPLAY").unwrap();
        let chatlogger = ChatLoggerBuilder::new();
        parse_replay(
            &std::path::PathBuf::from(input),
            game_dir,
            extracted,
            chatlogger,
        )
        .unwrap();
    }
    #[cfg(feature = "graphics")]
    {
        if let Some(matches) = matches.subcommand_matches("trace") {
            let input = matches.value_of("REPLAY").unwrap();
            let output = matches.value_of("out").unwrap();
            let trailer = analysis::trails::TrailsBuilder::new(output);
            parse_replay(
                &std::path::PathBuf::from(input),
                game_dir,
                extracted,
                trailer,
            )
            .unwrap();
        }
    }
    if let Some(matches) = matches.subcommand_matches("survey") {
        let mut survey_result = SurveyResults::empty();
        for replay in matches.values_of("REPLAYS").unwrap() {
            for entry in walkdir::WalkDir::new(replay) {
                let entry = entry.expect("Error unwrapping entry");
                if !entry.path().is_file() {
                    continue;
                }
                let replay = entry.path().to_path_buf();
                let result = survey_file(
                    matches.is_present("skip-decode"),
                    game_dir,
                    extracted,
                    replay,
                );
                survey_result.add(result);
            }
        }
        survey_result.print();
    }
    if let Some(matches) = matches.subcommand_matches("search") {
        let mut replays = vec![];
        for replay in matches.values_of("REPLAYS").unwrap() {
            for entry in walkdir::WalkDir::new(replay) {
                let entry = entry.expect("Error unwrapping entry");
                if !entry.path().is_file() {
                    continue;
                }
                let replay = entry.path().to_path_buf();
                let replay_path = replay.clone();

                let replay = match ReplayFile::from_file(&replay) {
                    Ok(replay) => replay,
                    Err(_) => {
                        continue;
                    }
                };
                replays.push((replay_path, replay.meta));

                if replays.len() % 100 == 0 {
                    println!("Parsed {} games...", replays.len());
                }

                //let result = survey_file(matches.is_present("skip-decode"), replay);
                //survey_result.add(result);
            }
        }
        replays.sort_by_key(|replay| {
            match chrono::NaiveDateTime::parse_from_str(&replay.1.dateTime, "%d.%m.%Y %H:%M:%S") {
                Ok(x) => x,
                Err(e) => {
                    println!("Couldn't parse '{}' because {:?}", replay.1.dateTime, e);
                    chrono::NaiveDateTime::parse_from_str(
                        "05.05.1995 01:02:03",
                        "%d.%m.%Y %H:%M:%S",
                    )
                    .unwrap()
                }
            }
            //replay.1.dateTime.clone()
        });
        println!("Found {} games", replays.len());
        for i in 0..10 {
            let idx = replays.len() - i - 1;
            println!(
                "{:?} {} {} {} {}",
                replays[idx].0,
                replays[idx].1.playerName,
                replays[idx].1.dateTime,
                replays[idx].1.mapDisplayName,
                replays[idx].1.playerVehicle
            );
        }
    }
}
