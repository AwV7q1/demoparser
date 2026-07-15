#![deny(clippy::all)]

#[macro_use]
extern crate napi_derive;
// ADR-007 roadmap bước 4, Giai đoạn 2 (cs2-analytics): compute_full_pipeline_async, napi::Task
// thật đầu tiên trong crate này -- xem full_pipeline.rs.
mod full_pipeline;
use ahash::AHashMap;
use memmap2::MmapOptions;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::JsValuesTupleIntoVec;
use napi::Either;
use napi::Env;
use napi::JsBigInt;
use napi::JsFunction;
use napi::JsObject;
use napi::JsUnknown;
use parser::first_pass::parser_settings::rm_map_user_friendly_names;
use parser::first_pass::parser_settings::rm_user_friendly_names;
use parser::first_pass::parser_settings::FirstPassParser;
use parser::first_pass::parser_settings::ParserInputs;
use parser::parse_demo::DemoOutput;
use parser::parse_demo::Parser;
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use parser::second_pass::parser_settings::RoundFlushChunk;
use parser::second_pass::parser_settings::SecondPassParser;
use parser::second_pass::variants::soa_to_aos;
use parser::second_pass::variants::BytesVariant;
use parser::second_pass::variants::OutputSerdeHelperStruct;
use parser::second_pass::variants::Variant;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::hash::RandomState;
use std::result::Result;

#[napi]
#[derive(Clone)]
pub struct JsVariant(Variant);

impl FromNapiValue for JsVariant {
  unsafe fn from_napi_value(env: sys::napi_env, napi_val: sys::napi_value) -> napi::Result<Self> {
    let js_unknown = JsUnknown::from_napi_value(env, napi_val)?;

    match js_unknown.get_type() {
      Ok(js_unknown_type) => {
        if js_unknown_type == ValueType::Boolean {
          if let Ok(val) = js_unknown.coerce_to_bool() {
            Ok(JsVariant(Variant::Bool(val.get_value()?)))
          } else {
            Err(Error::new(
              Status::InvalidArg,
              "Unspported Boolean type for Variant".to_owned(),
            ))
          }
        } else if js_unknown_type == ValueType::String {
          if let Ok(val) = js_unknown.coerce_to_string() {
            Ok(JsVariant(Variant::String(val.into_utf8()?.into_owned()?)))
          } else {
            Err(Error::new(
              Status::InvalidArg,
              "Unsupported String for Variant".to_owned(),
            ))
          }
        } else if js_unknown_type == ValueType::Number {
          if let Ok(val) = js_unknown.coerce_to_number() {
            let num = val.get_double()?;
            if num.fract() == 0.0 {
              if num >= u8::MIN as f64 && num <= u8::MAX as f64 {
                Ok(JsVariant(Variant::I32(num as i32)))
              } else if let Ok(val) = val.get_int32() {
                let int32_val = val;
                if int32_val >= i16::MIN as i32 && int32_val <= i16::MAX as i32 {
                  Ok(JsVariant(Variant::I32(int32_val)))
                } else {
                  Ok(JsVariant(Variant::I32(int32_val)))
                }
              } else if let Ok(val) = val.get_uint32() {
                Ok(JsVariant(Variant::U32(val)))
              } else {
                Err(Error::new(
                  Status::InvalidArg,
                  "Unsupported number type".to_owned(),
                ))
              }
            } else {
              Ok(JsVariant(Variant::F32(num as f32)))
            }
          } else {
            Err(Error::new(
              Status::InvalidArg,
              "Unsupported number type".to_owned(),
            ))
          }
        } else if js_unknown_type == ValueType::BigInt {
          let bigint_val = js_unknown.cast::<JsBigInt>();
          match bigint_val.get_u64() {
            Ok((val, true)) => Ok(JsVariant(Variant::U64(val))),
            _ => Err(Error::new(
              Status::InvalidArg,
              "Unsupported number type".to_owned(),
            )),
          }
        } else {
          Err(Error::new(
            Status::InvalidArg,
            "Unspported type for Variant".to_owned(),
          ))
        }
      }
      _ => Err(Error::new(
        Status::InvalidArg,
        "Unspported type for Variant".to_owned(),
      )),
    }
  }
}

#[napi]
pub struct WantedPropState {
  pub prop: String,
  pub state: JsVariant,
}

impl FromNapiValue for WantedPropState {
  unsafe fn from_napi_value(
    env: sys::napi_env,
    napi_val: napi::sys::napi_value,
  ) -> napi::Result<Self> {
    let obj: Object = Object::from_napi_value(env, napi_val)?;

    let prop: String = obj.get_named_property("prop")?;
    let state: JsVariant = obj.get_named_property("state")?;

    Ok(WantedPropState { prop, state })
  }
}

fn parse_demo(bytes: BytesVariant, parser: &mut Parser) -> Result<DemoOutput, Error> {
  match bytes {
    BytesVariant::Mmap(m) => match parser.parse_demo(&m) {
      Ok(output) => Ok(output),
      Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    },
    BytesVariant::Vec(v) => match parser.parse_demo(&v) {
      Ok(output) => Ok(output),
      Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    },
  }
}
#[napi(object)]
pub struct VoiceData {
  pub tick: i32,
  pub data: Buffer,
  pub steamid: String,
}

#[napi]
pub fn parse_voice(path_or_buf: Either<String, Buffer>) -> napi::Result<Vec<VoiceData>> {
  let bytes = resolve_byte_type(path_or_buf).unwrap();
  let settings = ParserInputs {
    wanted_players: vec![],
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_ticks: vec![],
    wanted_prop_states: AHashMap::default(),
    real_name_to_og_name: AHashMap::default(),
    parse_ents: false,
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &vec![],
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let mut out = vec![];

  for (tick, packet) in output.voice_data {
    if let Some(data) = &packet.audio {
      out.push(VoiceData {
        data: data.voice_data().into(),
        steamid: packet.xuid().to_string(),
        tick: tick,
      });
    }
  }
  return Ok(out);
}

#[napi]
pub fn list_game_events(path_or_buf: Either<String, Buffer>) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;

  let huf = create_huffman_lookup_table();
  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec!["all".to_string()],
    parse_ents: false,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;

  let v = Vec::from_iter(output.game_events_counter.iter());
  let s = match serde_json::to_value(v) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}
/// extra: lets you add new fields to grenades. Use list_updated_fields for a full list.
/// grenades: lets you disable non-projectile grenades. This can have a big difference on memory/speed.
#[napi]
pub fn parse_grenades(
  path_or_buf: Either<String, Buffer>,
  extra: Option<Vec<String>>,
  grenades: Option<bool>,
) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();
  let mut extra_props = match extra {
    Some(p) => p,
    None => vec![],
  };
  let real_extra_props = match rm_user_friendly_names(&extra_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let grenades = grenades.unwrap_or(true);

  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: real_extra_props.clone(),
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: true,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: grenades,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;

  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_extra_props.iter().zip(&extra_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  extra_props.push("tick".to_owned());
  extra_props.push("steamid".to_owned());
  extra_props.push("name".to_owned());

  let mut prop_infos = output.prop_controller.prop_infos.clone();
  prop_infos.sort_by_key(|x| x.prop_name.clone());
  extra_props.sort();

  let helper = OutputSerdeHelperStruct {
    prop_infos: prop_infos.clone(),
    inner: output.df.clone().into(),
  };
  let result = soa_to_aos(helper);
  match serde_json::to_value(&result) {
    Ok(s) => Ok(s),
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  }
}
#[napi]
pub fn parse_header(path_or_buf: Either<String, Buffer>) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    real_name_to_og_name: AHashMap::default(),
    wanted_players: vec![],
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![],
    parse_ents: false,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = FirstPassParser::new(&settings);
  let output = match bytes {
    BytesVariant::Mmap(m) => match parser.parse_header_only(&m) {
      Ok(output) => Ok(output),
      Err(e) => Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    },
    BytesVariant::Vec(v) => match parser.parse_header_only(&v) {
      Ok(output) => Ok(output),
      Err(e) => Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    },
  }?;

  let mut hm: HashMap<String, String> = HashMap::default();
  hm.extend(output);

  let s = match serde_json::to_value(&hm) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}

#[napi]
pub fn parse_event(
  path_or_buf: Either<String, Buffer>,
  event_name: String,
  player_extra: Option<Vec<String>>,
  other_extra: Option<Vec<String>>,
  game_event_list_bytes: Option<Buffer>,
) -> napi::Result<Value> {
  let player_props = match player_extra {
    Some(p) => p,
    None => vec![],
  };
  let other_props = match other_extra {
    Some(p) => p,
    None => vec![],
  };
  let real_names_player = match rm_user_friendly_names(&player_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let real_other_props = match rm_user_friendly_names(&other_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };

  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names_player.iter().zip(&player_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }
  for (real_name, user_friendly_name) in real_other_props.iter().zip(&other_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let game_event_list_bytes = if let Some(b) = game_event_list_bytes {
    Some(b.to_vec())
  } else {
    None
  };

  let settings = ParserInputs {
    real_name_to_og_name: real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names_player.clone(),
    wanted_other_props: real_other_props,
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![event_name],
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: game_event_list_bytes,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let s = match serde_json::to_value(&output.game_events) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}
#[napi]
pub fn parse_events(
  path_or_buf: Either<String, Buffer>,
  event_names: Option<Vec<String>>,
  player_extra: Option<Vec<String>>,
  other_extra: Option<Vec<String>>,
  game_event_list_bytes: Option<Buffer>,
) -> napi::Result<Value> {
  let event_names = match event_names {
    None => return Err(Error::new(Status::InvalidArg, "No events provided!")),
    Some(v) => v,
  };
  let player_props = match player_extra {
    Some(p) => p,
    None => vec![],
  };
  let other_props = match other_extra {
    Some(p) => p,
    None => vec![],
  };
  let real_names_player = match rm_user_friendly_names(&player_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let real_other_props = match rm_user_friendly_names(&other_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };

  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names_player.iter().zip(&player_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }
  for (real_name, user_friendly_name) in real_other_props.iter().zip(&other_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let game_event_list_bytes = if let Some(b) = game_event_list_bytes {
    Some(b.to_vec())
  } else {
    None
  };

  let settings = ParserInputs {
    real_name_to_og_name: real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names_player.clone(),
    wanted_other_props: real_other_props.clone(),
    wanted_prop_states: AHashMap::default(),
    wanted_events: event_names,
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: game_event_list_bytes,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let s = match serde_json::to_value(&output.game_events) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}

#[napi]
pub fn parse_ticks(
  path_or_buf: Either<String, Buffer>,
  wanted_props: Vec<String>,
  wanted_ticks: Option<Vec<i32>>,
  wanted_players: Option<Vec<String>>,
  struct_of_arrays: Option<bool>,
  order_by_steamid: Option<bool>,
  prop_states: Option<Vec<WantedPropState>>,
) -> napi::Result<Value> {
  let mut real_names = match rm_user_friendly_names(&wanted_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let wanted_players_u64 = match wanted_players {
    Some(v) => v.iter().map(|x| x.parse::<u64>().unwrap_or(0)).collect(),
    None => vec![],
  };
  let wanted_prop_states: AHashMap<String, Variant> = prop_states
    .unwrap_or_default()
    .into_iter()
    .map(|prop| (prop.prop.clone(), prop.state.0.clone()))
    .collect();

  let real_wanted_prop_states = rm_map_user_friendly_names(&wanted_prop_states);
  let real_wanted_prop_states = match real_wanted_prop_states {
    Ok(real_wanted_prop_states) => real_wanted_prop_states,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };

  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();
  let mut real_name_to_og_name = AHashMap::default();

  for (real_name, user_friendly_name) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }
  for (real_name, user_friendly_name) in real_wanted_prop_states
    .keys()
    .zip(wanted_prop_states.keys())
  {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  let wanted_ticks = match wanted_ticks {
    Some(t) => t,
    None => vec![],
  };
  let order_by_steamid = match order_by_steamid {
    Some(true) => true,
    _ => false,
  };

  let settings = ParserInputs {
    real_name_to_og_name: real_name_to_og_name,
    wanted_players: wanted_players_u64,
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: real_wanted_prop_states,
    parse_ents: true,
    wanted_ticks: wanted_ticks,
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: order_by_steamid,
    fallback_bytes: None,
    parse_grenades: false,
  };

  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  real_names.push("tick".to_owned());
  real_names.push("steamid".to_owned());
  real_names.push("name".to_owned());

  let mut prop_infos = output.prop_controller.prop_infos.clone();
  prop_infos.sort_by_key(|x| x.prop_name.clone());
  real_names.sort();

  let helper = OutputSerdeHelperStruct {
    prop_infos: prop_infos.clone(),
    inner: output.df.clone().into(),
  };

  let is_soa = match struct_of_arrays {
    Some(true) => true,
    _ => false,
  };

  if order_by_steamid {
    let mut helper_hm: HashMap<u64, _, RandomState> = HashMap::default();
    for (k, v) in output.df_per_player {
      let helper = OutputSerdeHelperStruct {
        prop_infos: prop_infos.clone(),
        inner: v.into(),
      };
      helper_hm.insert(k, helper);
    }
    let s = match serde_json::to_value(helper_hm) {
      Ok(s) => s,
      Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    };
    return Ok(s);
  }
  if is_soa {
    let s = match serde_json::to_value(&helper) {
      Ok(s) => s,
      Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    };
    return Ok(s);
  } else {
    let result = soa_to_aos(helper);
    let s = match serde_json::to_value(&result) {
      Ok(s) => s,
      Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
    };
    Ok(s)
  }
}

/// ADR-007 §VI.2 parity-debug ONLY: identical to `parse_ticks` but forces
/// `ParsingMode::ForceSingleThreaded` -- used to isolate whether a Rust-streaming-vs-JS-bulk
/// mismatch is a real codec bug or an artifact of comparing ST (streaming) against MT (bulk's
/// default ParsingMode::Normal, which resolves to multi-threaded for most prop sets).
#[napi]
pub fn parse_ticks_force_st(
  path_or_buf: Either<String, Buffer>,
  wanted_props: Vec<String>,
) -> napi::Result<Value> {
  let real_names = match rm_user_friendly_names(&wanted_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::ForceSingleThreaded);
  let output = parse_demo(bytes, &mut parser)?;
  let helper = OutputSerdeHelperStruct {
    prop_infos: output.prop_controller.prop_infos.clone(),
    inner: output.df.clone().into(),
  };
  let s = match serde_json::to_value(&helper) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}

/// ADR-007 Giai đoạn 3 parity-debug ONLY: like `parse_ticks` but forces
/// `ParsingMode::ForceSingleThreaded` AND honors `wanted_ticks` (the sampled subset). Mirrors the
/// full-pipeline's internal `run_parse_ticks_raw` exactly, so a TS `computeMatchData` baseline can
/// be fed the SAME ST sampled ticks the Rust pipeline uses -> replay-chunk/stats parity is
/// apples-to-apples (not ST-pipeline vs MT-default-bulk). Prop_infos sorted by prop_name to match.
#[napi]
pub fn parse_ticks_sampled_st(
  path_or_buf: Either<String, Buffer>,
  wanted_props: Vec<String>,
  wanted_ticks: Vec<i32>,
  struct_of_arrays: Option<bool>,
) -> napi::Result<Value> {
  let real_names = match rm_user_friendly_names(&wanted_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks,
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::ForceSingleThreaded);
  let output = parse_demo(bytes, &mut parser)?;
  let mut prop_infos = output.prop_controller.prop_infos.clone();
  prop_infos.sort_by_key(|x| x.prop_name.clone());
  let helper = OutputSerdeHelperStruct { prop_infos, inner: output.df.clone().into() };
  let result = if struct_of_arrays.unwrap_or(false) {
    serde_json::to_value(&helper)
  } else {
    serde_json::to_value(&soa_to_aos(helper))
  };
  result.map_err(|e| Error::new(Status::InvalidArg, format!("{}", e).to_owned()))
}

/// ADR-007 §VI.2 streaming prototype (cs2-analytics): per-round variant of `parse_ticks`.
/// Instead of returning one bulk result, invokes `callback` once per round with
/// `{ tick, rows, events, bytes }` -- `bytes` is that round's actual columnar prop data
/// (reusing the same `OutputSerdeHelperStruct`/SoA serialization `parse_ticks` already uses,
/// just JSON-encoded rather than zstd -- the real columnar+quantize+zstd codec lives in
/// TS/`packages/shared` and is already parity-verified separately; JSON here is only to put a
/// REAL per-round byte payload across the N-API boundary instead of a 3-number summary) --
/// then drops that round's decoded props/events immediately after. Forces single-threaded
/// second pass (see ADR-007 for why streaming requires it: the default multi-threaded path
/// parallelizes across demo segments with no sequential round boundary). Runs synchronously on
/// the calling JS thread (same blocking behavior as `parse_ticks` today) -- not yet using a
/// threadsafe_function/background thread, since the goal here is only to validate that the
/// callback path + RAM behavior hold up from Node, not to make it async.
#[napi]
pub fn parse_ticks_streaming(
  env: Env,
  path_or_buf: Either<String, Buffer>,
  wanted_props: Vec<String>,
  callback: JsFunction,
) -> napi::Result<Value> {
  let real_names = match rm_user_friendly_names(&wanted_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    // MUST stay empty: collect_data.rs's collect_entities() skips prop collection entirely
    // when wanted_events is non-empty. Round-boundary detection does not depend on
    // wanted_events (see second_pass/entities.rs) so this is safe.
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };

  let mut first_pass_parser = FirstPassParser::new(&settings);
  let first_pass_output = match first_pass_parser.parse_demo(&bytes[..], false) {
    Ok(o) => o,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let prop_infos = first_pass_output.prop_controller.prop_infos.clone();
  let name_to_id = parser::tick_codec::build_name_to_id(&prop_infos);

  let cb: Box<dyn FnMut(RoundFlushChunk)> = Box::new(move |chunk: RoundFlushChunk| {
    let rows: i64 = chunk.output.values().map(|c| c.len() as i64).sum();
    let events = chunk.game_events.len() as i64;
    // Real pre-zstd payload -- byte-layout-identical to replay-codec-core.ts's
    // encodeReplayChunkBody (see tick_codec.rs). zstd itself happens on the Node side via the
    // existing zstdCompress() helper (no C toolchain here to build zstd-sys).
    let payload_bytes = parser::tick_codec::encode_round_tick_body(&chunk, &name_to_id);
    if let Ok(mut obj) = env.create_object() {
      let _ = obj.set("tick", chunk.tick);
      let _ = obj.set("rows", rows);
      let _ = obj.set("events", events);
      let _ = obj.set("bytes", Buffer::from(payload_bytes));
      let _ = callback.call(None, &[obj.into_unknown()]);
    }
    // payload_bytes drops here (Buffer already handed its Vec<u8> to JS) -> this round's
    // decoded props freed before the next round starts
  });

  let mut second_pass = match SecondPassParser::new(first_pass_output.clone(), 16, true, None) {
    Ok(p) => p.with_round_flush(cb),
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  if let Err(e) = second_pass.start(&bytes[..]) {
    return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned()));
  }

  let tail_rows: i64 = second_pass.output.values().map(|c| c.len() as i64).sum();
  let tail_events = second_pass.game_events.len() as i64;
  Ok(serde_json::json!({ "tailRows": tail_rows, "tailEvents": tail_events }))
}

/// ADR-007 §VI.2 follow-up: does the ~150MB Node/V8 overhead go away if the round payload never
/// crosses N-API as a `Buffer` at all? Same encode as `parse_ticks_streaming`, but each round's
/// bytes are written straight to `out_path` from Rust (plain `std::fs::File`, appended); the JS
/// callback -- still invoked once per round, since the round-boundary/manifest bookkeeping is
/// the interesting part to keep -- only receives numbers (tick/rows/events/offset/len), never a
/// Buffer. No `env.create_object()`+`Buffer::from()` per round, so nothing round-sized should be
/// left for V8's GC to reclaim.
#[napi]
pub fn parse_ticks_streaming_to_file(
  env: Env,
  path_or_buf: Either<String, Buffer>,
  wanted_props: Vec<String>,
  out_path: String,
  callback: JsFunction,
) -> napi::Result<Value> {
  use std::io::Write;

  let real_names = match rm_user_friendly_names(&wanted_props) {
    Ok(names) => names,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, user_friendly_name) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), user_friendly_name.clone());
  }

  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };

  let mut first_pass_parser = FirstPassParser::new(&settings);
  let first_pass_output = match first_pass_parser.parse_demo(&bytes[..], false) {
    Ok(o) => o,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  let prop_infos = first_pass_output.prop_controller.prop_infos.clone();
  let name_to_id = parser::tick_codec::build_name_to_id(&prop_infos);

  let mut out_file = match File::create(&out_path) {
    Ok(f) => f,
    Err(e) => return Err(Error::new(Status::GenericFailure, format!("create {out_path}: {e}"))),
  };
  // i64 is Copy, so a plain `let mut cursor` captured by a `move` closure would only hand the
  // closure its own copy -- the outer binding would never see the closure's increments. Rc<Cell>
  // shares the same cell instead, so the final read below sees the real total.
  let cursor = std::rc::Rc::new(std::cell::Cell::new(0i64));
  let cursor_cb = cursor.clone();

  let cb: Box<dyn FnMut(RoundFlushChunk)> = Box::new(move |chunk: RoundFlushChunk| {
    let rows: i64 = chunk.output.values().map(|c| c.len() as i64).sum();
    let events = chunk.game_events.len() as i64;
    let tick = chunk.tick;
    let payload_bytes = parser::tick_codec::encode_round_tick_body(&chunk, &name_to_id);
    let raw_len = payload_bytes.len() as i64;
    // Level 3 matches Node zlib's zstdCompressSync default (ZSTD_CLEVEL_DEFAULT) -- on-disk blob
    // size stays the same as the current Node-side-compression production behavior.
    let compressed = parser::zstd_codec::compress(&payload_bytes, 3).unwrap_or(payload_bytes);
    let len = compressed.len() as i64;
    let offset = cursor_cb.get();
    if out_file.write_all(&compressed).is_ok() {
      cursor_cb.set(offset + len);
    }
    // compressed/payload_bytes drop here -- plain Vec<u8>, never touched V8/N-API at all.
    if let Ok(mut obj) = env.create_object() {
      let _ = obj.set("tick", tick);
      let _ = obj.set("rows", rows);
      let _ = obj.set("events", events);
      let _ = obj.set("offset", offset);
      let _ = obj.set("len", len);
      let _ = obj.set("rawLen", raw_len);
      let _ = callback.call(None, &[obj.into_unknown()]);
    }
  });

  let mut second_pass = match SecondPassParser::new(first_pass_output.clone(), 16, true, None) {
    Ok(p) => p.with_round_flush(cb),
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  if let Err(e) = second_pass.start(&bytes[..]) {
    return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned()));
  }

  let tail_rows: i64 = second_pass.output.values().map(|c| c.len() as i64).sum();
  let tail_events = second_pass.game_events.len() as i64;
  Ok(serde_json::json!({ "tailRows": tail_rows, "tailEvents": tail_events, "totalBytes": cursor.get() }))
}

#[napi]
pub fn parse_player_info(path_or_buf: Either<String, Buffer>) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![],
    parse_ents: false,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let s = match serde_json::to_value(&output.player_md) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}

#[napi]
pub fn parse_player_skins(path_or_buf: Either<String, Buffer>) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![],
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let s = match serde_json::to_value(&output.skins) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}
// ADR-007 §VI.2 (cs2-analytics) "events" domain port. Second stage, orthogonal to every parse_*
// function above: takes the SAME shape Node already has today from parse_events()/parse_grenades()
// (or from @laihoe/demoparser2 in production) and computes rounds/events/replay-event-chunk blobs
// -- pure compute logic ported from packages/parse-core/src/compute.ts's "events" section, see
// src/parser/src/compute_events/. Does NOT touch demo parsing itself (no new parse_ticks*
// variant, no change to any function above).
#[napi]
pub fn compute_events(
  env: Env,
  all_events: Value,
  grenade_rows: Value,
  zstd_level: Option<i32>,
) -> napi::Result<JsObject> {
  let events_in: Vec<parser::compute_events::RawEvent> = match serde_json::from_value(all_events) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("all_events: {e}"))),
  };
  let grenade_in: Vec<parser::compute_events::RawGrenadeSample> = match serde_json::from_value(grenade_rows) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("grenade_rows: {e}"))),
  };
  let level = zstd_level.unwrap_or(3);
  let result = parser::compute_events::compute_events(&events_in, &grenade_in, level);

  let rounds_val = match serde_json::to_value(&result.rounds) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("rounds serialize: {e}"))),
  };

  let mut chunks_arr = env.create_array_with_length(result.replay_event_chunks.len())?;
  for (i, c) in result.replay_event_chunks.iter().enumerate() {
    let mut chunk_obj = env.create_object()?;
    chunk_obj.set("roundNumber", c.round_number)?;
    chunk_obj.set("format", c.format)?;
    chunk_obj.set("eventCount", c.event_count)?;
    chunk_obj.set("data", Buffer::from(c.data.clone()))?;
    chunks_arr.set_element(i as u32, chunk_obj)?;
  }

  let mut out = env.create_object()?;
  out.set("rounds", rounds_val)?;
  out.set("events", Value::Array(result.events))?;
  out.set("replayEventChunks", chunks_arr)?;
  Ok(out)
}

// ADR-007 §VI.2 (cs2-analytics) "stats" domain port. Independent of compute_events at the N-API
// boundary -- kills_batch/weapon_fire_batch/hurt_batch are dumped from the SAME shape TS's own
// killsBatch/weaponFireBatch/hurtBatch already have (i.e. compute.ts's own buildKills/
// buildWeaponFire/buildHurt output), not chained through this addon's compute_events. raw_kills/
// raw_hurt/player_info/tick_data are the other raw inputs computePlayerStats/computeTickAggregates
// read directly. See src/parser/src/compute_stats/.
#[napi]
#[allow(clippy::too_many_arguments)]
pub fn compute_stats(
  kills_batch: Value,
  weapon_fire_batch: Value,
  hurt_batch: Value,
  raw_kills: Value,
  raw_hurt: Value,
  player_info: Value,
  tick_data: Value,
  rounds: Value,
) -> napi::Result<Value> {
  macro_rules! parse {
    ($field:expr, $ty:ty, $name:literal) => {
      match serde_json::from_value::<$ty>($field) {
        Ok(v) => v,
        Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}: {e}", $name))),
      }
    };
  }
  let kills_batch = parse!(kills_batch, Vec<parser::compute_stats::KillsBatchItem>, "kills_batch");
  let weapon_fire_batch = parse!(weapon_fire_batch, Vec<parser::compute_stats::WeaponFireBatchItem>, "weapon_fire_batch");
  let hurt_batch = parse!(hurt_batch, Vec<parser::compute_stats::HurtBatchItem>, "hurt_batch");
  let raw_kills = parse!(raw_kills, Vec<parser::compute_stats::RawKillRow>, "raw_kills");
  let raw_hurt = parse!(raw_hurt, Vec<parser::compute_stats::RawHurtRow>, "raw_hurt");
  let player_info = parse!(player_info, Vec<parser::compute_stats::RawPlayerInfo>, "player_info");
  let rounds = parse!(rounds, Vec<parser::compute_events::ParsedRound>, "rounds");

  let result = parser::compute_stats::compute_stats(
    &kills_batch, &weapon_fire_batch, &hurt_batch, &raw_kills, &raw_hurt, &player_info, &tick_data, &rounds,
  );

  let out = serde_json::json!({
    "matchWeaponStats": result.match_weapon_stats,
    "playerAccuracyStats": result.player_accuracy_stats,
    "playerMatchStats": result.player_match_stats,
    "roundSurvivorStats": result.round_survivor_stats,
    "playerZoneStats": result.player_zone_stats,
    "roundEconomyStats": result.round_economy_stats,
    "roundPlayerDamageStats": result.round_player_damage_stats,
  });
  Ok(out)
}

// ADR-007 §VI.2 (cs2-analytics) "aim" domain port. See src/parser/src/compute_aim/ -- takes
// AIM_TICK_FIELDS rows already fetched for the same kill-window ticks computeAimStats itself
// would have asked parser.parseTicks() for (this addon does not call the parser here).
#[napi]
pub fn compute_aim_stats(kill_events: Value, weapon_fire_batch: Value, aim_tick_rows: Value) -> napi::Result<Value> {
  let kill_events: Vec<parser::compute_aim::RawAimKillRow> = match serde_json::from_value(kill_events) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("kill_events: {e}"))),
  };
  let weapon_fire_batch: Vec<parser::compute_stats::WeaponFireBatchItem> = match serde_json::from_value(weapon_fire_batch) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("weapon_fire_batch: {e}"))),
  };
  let aim_tick_rows: Vec<parser::compute_aim::RawAimTickRow> = match serde_json::from_value(aim_tick_rows) {
    Ok(v) => v,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("aim_tick_rows: {e}"))),
  };
  let result = parser::compute_aim::compute_aim_stats(&kill_events, &weapon_fire_batch, &aim_tick_rows);
  match serde_json::to_value(&result) {
    Ok(v) => Ok(v),
    Err(e) => Err(Error::new(Status::InvalidArg, format!("{}", e))),
  }
}

#[napi]
pub fn list_updated_fields(path_or_buf: Either<String, Buffer>) -> napi::Result<Value> {
  let bytes = resolve_byte_type(path_or_buf)?;
  let huf = create_huffman_lookup_table();

  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec!["none".to_string()],
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: false,
    list_props: true,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, parser::parse_demo::ParsingMode::Normal);
  let output = parse_demo(bytes, &mut parser)?;
  let s = match serde_json::to_value(&output.uniq_prop_names) {
    Ok(s) => s,
    Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
  };
  Ok(s)
}

fn resolve_byte_type(path_or_buf: Either<String, Buffer>) -> Result<BytesVariant, napi::Error> {
  match path_or_buf {
    Either::A(path) => {
      let file = match File::open(path.clone()) {
        Ok(f) => f,
        Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
      };
      let mmap = match unsafe { MmapOptions::new().map(&file) } {
        Ok(mmap) => mmap,
        Err(e) => return Err(Error::new(Status::InvalidArg, format!("{}", e).to_owned())),
      };
      Ok(BytesVariant::Mmap(mmap))
    }
    Either::B(buf) => Ok(BytesVariant::Vec(buf.into())),
  }
}
