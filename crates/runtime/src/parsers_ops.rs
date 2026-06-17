//! Host ops backing the `runtime:parsers` module for XML processing.
use es_runtime_engine::{Engine, OpDecl, Value};
use quick_xml::events::Event;
use quick_xml::Reader;

fn json_to_value(v: serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Value::Number(f)
            } else {
                Value::Number(f64::NAN)
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => {
            Value::Array(arr.into_iter().map(json_to_value).collect())
        }
        serde_json::Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, json_to_value(v))).collect())
        }
    }
}

fn value_to_json(v: Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::Number(n) => serde_json::Number::from_f64(n).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s),
        Value::Array(arr) => serde_json::Value::Array(arr.into_iter().map(value_to_json).collect()),
        Value::Object(map) => {
            let mut obj = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                obj.insert(k, value_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::Null,
    }
}

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    engine.register_op(OpDecl::sync("xml_parse", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        fn parse_node(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>, current_tag: Option<&[u8]>) -> Result<Value, String> {
            let mut map: Vec<(String, Value)> = Vec::new();
            let mut text_content = String::new();

            loop {
                match reader.read_event_into(buf) {
                    Ok(Event::Start(ref e)) => {
                        let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        
                        // Parse attributes
                        let mut child_map = Vec::new();
                        for attr in e.attributes() {
                            if let Ok(a) = attr {
                                let k = format!("@{}", String::from_utf8_lossy(a.key.as_ref()));
                                let v = String::from_utf8_lossy(a.value.as_ref()).to_string();
                                child_map.push((k, Value::String(v)));
                            }
                        }

                        // Recursively parse child
                        match parse_node(reader, &mut Vec::new(), Some(e.name().as_ref())) {
                            Ok(Value::Object(mut props)) => {
                                child_map.append(&mut props);
                            }
                            Ok(Value::String(s)) => {
                                if !s.is_empty() {
                                    child_map.push(("$text".to_string(), Value::String(s)));
                                }
                            }
                            _ => {}
                        }

                        let child_val = Value::Object(child_map);

                        // If key exists, convert to Array or append to Array
                        if let Some(existing) = map.iter_mut().find(|(k, _)| k == &name) {
                            match &mut existing.1 {
                                Value::Array(arr) => arr.push(child_val),
                                _ => {
                                    // Take the old value and wrap it in an array along with the new value
                                    let old_val = std::mem::replace(&mut existing.1, Value::Null);
                                    existing.1 = Value::Array(vec![old_val, child_val]);
                                }
                            }
                        } else {
                            map.push((name, child_val));
                        }
                    }
                    Ok(Event::Empty(ref e)) => {
                        let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                        let mut child_map = Vec::new();
                        for attr in e.attributes() {
                            if let Ok(a) = attr {
                                let k = format!("@{}", String::from_utf8_lossy(a.key.as_ref()));
                                let v = String::from_utf8_lossy(a.value.as_ref()).to_string();
                                child_map.push((k, Value::String(v)));
                            }
                        }
                        
                        let child_val = if child_map.is_empty() {
                            Value::String("".to_string())
                        } else {
                            Value::Object(child_map)
                        };

                        if let Some(existing) = map.iter_mut().find(|(k, _)| k == &name) {
                            match &mut existing.1 {
                                Value::Array(arr) => arr.push(child_val),
                                _ => {
                                    let old_val = std::mem::replace(&mut existing.1, Value::Null);
                                    existing.1 = Value::Array(vec![old_val, child_val]);
                                }
                            }
                        } else {
                            map.push((name, child_val));
                        }
                    }
                    Ok(Event::Text(e)) => {
                        let text_str = String::from_utf8_lossy(e.as_ref());
                        if let Ok(unescaped) = quick_xml::escape::unescape(&text_str) {
                            text_content.push_str(&unescaped);
                        } else {
                            text_content.push_str(&text_str);
                        }
                    }
                    Ok(Event::End(ref e)) => {
                        if Some(e.name().as_ref()) == current_tag {
                            break;
                        }
                    }
                    Ok(Event::Eof) => break,
                    Err(e) => return Err(format!("Parse failed: {}", e)),
                    _ => {}
                }
            }

            if map.is_empty() && !text_content.is_empty() {
                Ok(Value::String(text_content))
            } else {
                if !text_content.trim().is_empty() {
                    map.push(("$text".to_string(), Value::String(text_content.trim().to_string())));
                }
                Ok(Value::Object(map))
            }
        }

        match parse_node(&mut reader, &mut buf, None) {
            Ok(v) => Ok(v),
            Err(e) => Ok(Value::String(e.into()))
        }
    }))?;
    
    engine.register_op(OpDecl::sync("xml_validate", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        
        let mut reader = Reader::from_str(xml);
        loop {
            match reader.read_event() {
                Ok(Event::Eof) => break,
                Err(e) => return Ok(Value::String(format!("Validation failed: {}", e).into())),
                _ => (),
            }
        }
        
        Ok(Value::Bool(true))
    }))?;
    
    engine.register_op(OpDecl::sync("xml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        let val = value_to_json(val);
        
        // Determine root tag: if it's an object with a single key, use it as root.
        // Otherwise use "root".
        let (root_name, inner_val) = match val.as_object() {
            Some(obj) if obj.len() == 1 => {
                let (k, v) = obj.into_iter().next().unwrap();
                (k.clone(), v.clone())
            }
            _ => ("root".to_string(), val),
        };
        
        match quick_xml::se::to_string_with_root(&root_name, &inner_val) {
            Ok(xml_str) => Ok(Value::String(xml_str.into())),
            Err(e) => Ok(Value::String(format!("Build failed: {}", e).into())),
        }
    }))?;
    Ok(())
}
