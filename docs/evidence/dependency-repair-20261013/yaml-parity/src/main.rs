use serde::{Deserialize, Serialize};
use std::error::Error;
#[derive(Debug, Serialize, Deserialize, PartialEq)]
enum Tagged { Unit, Newtype(String), Tuple(u64, String), Struct { content: String } }
fn main() -> Result<(), Box<dyn Error>> {
    let documents = [
        "name: 中文😀\nmetadata: {purpose: '签名', nullable: null}\n",
        "mapping: {true: yes, count: 42, float: 2.5}\nlist: [1, null, false, 中文]\n",
        "template: &content {title: 中文😀, count: 3}\ncopy: *content\n",
        "defaults: &base {name: 中文😀, count: 3}\nmerged: {<<: *base, other: null}\n",
        "null: null\nempty:\nquoted: 'null'\n",
        "value: !!str 42\nother: !!int 42\n",
        "? [one, two]\n: composite-key\n",
    ];
    for input in documents {
        let old: legacy::Value = legacy::from_str(input)?;
        let new: replacement::Value = replacement::from_str(input)?;
        assert_eq!(legacy::to_string(&old)?, replacement::to_string(&new)?);
        let old_round: legacy::Value = legacy::from_str(&legacy::to_string(&old)?)?;
        let new_round: replacement::Value = replacement::from_str(&replacement::to_string(&new)?)?;
        assert_eq!(old, old_round); assert_eq!(new, new_round);
    }
    for input in ["!Unit", "!Newtype 中文😀", "!Tuple [42, 中文😀]", "!Struct {content: 中文😀}"] {
        let old: Tagged = legacy::from_str(input)?; let new: Tagged = replacement::from_str(input)?;
        assert_eq!(old,new); assert_eq!(legacy::to_string(&old)?,replacement::to_string(&new)?);
        let old_value: legacy::Value=legacy::from_str(input)?;
        let new_value: replacement::Value=replacement::from_str(input)?;
        assert_eq!(legacy::to_string(&old_value)?,replacement::to_string(&new_value)?);
    }
    for invalid in ["name: [unclosed", "anchor: *missing", "name: first\nname: duplicate", "{中文: [}"] {
        let old=legacy::from_str::<legacy::Value>(invalid).err().ok_or("legacy parser accepted malformed YAML")?;
        let new=replacement::from_str::<replacement::Value>(invalid).err().ok_or("replacement parser accepted malformed YAML")?;
        assert!(old.location().is_some()); assert!(new.location().is_some());
        assert_eq!(old.to_string(),new.to_string());
    }
    println!("YAML API parity: 7 Value/Mapping/anchors/null/UTF8 documents, 4 typed/tagged variants, 4 exact errors; roundtrip and serialization bytes match");
    Ok(())
}
