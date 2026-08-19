use echo_rust_learning::serialization::{
    AgentProfile, ProfileError, parse_and_validate_profile, profile_to_json,
};

fn main() -> Result<(), ProfileError> {
    let profile = AgentProfile {
        name: "assistant".to_string(),
        max_iterations: 8,
        tools: vec!["search".to_string(), "read_file".to_string()],
    };
    let json = profile_to_json(&profile)?;
    let decoded = parse_and_validate_profile(&json)?;

    println!("JSON:\n{json}");
    println!("往返一致: {}", decoded == profile);
    Ok(())
}
