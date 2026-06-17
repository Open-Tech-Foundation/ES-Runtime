//! Host ops backing the `runtime:parsers` module for XML processing.
use es_runtime_engine::{Engine, OpDecl, Value};
use quick_xml::events::Event;
use quick_xml::Reader;

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    engine.register_op(OpDecl::sync("xml_parse", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        
        match quick_xml::de::from_str::<serde_json::Value>(xml) {
            Ok(val) => {
                let json_str = serde_json::to_string(&val).unwrap_or_default();
                Ok(Value::String(json_str.into()))
            }
            Err(e) => Ok(Value::String(format!("Parse failed: {}", e).into())),
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
    
    engine.register_op(OpDecl::sync("xml_build", |args| {
        let json_str = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        
        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(val) => {
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
            }
            Err(e) => Ok(Value::String(format!("Parse failed: {}", e).into())),
        }
    }))?;
    Ok(())
}
