use jsonschema::validator_for;
use serde_json::Value;

use crate::tools::traits::ToolError;

pub fn validate(schema: &Value, input: &Value) -> Result<(), ToolError> {
    let compiled =
        validator_for(schema).map_err(|err| ToolError::SchemaValidation(err.to_string()))?;
    if let Err(error) = compiled.validate(input) {
        let detail = error.to_string();
        return Err(ToolError::InvalidInput(detail));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::validate;

    #[test]
    fn validate_accepts_valid_input() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" }
            },
            "additionalProperties": false
        });
        let input = json!({"name": "pico"});

        assert!(validate(&schema, &input).is_ok());
    }

    #[test]
    fn validate_rejects_invalid_input() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" }
            },
            "additionalProperties": false
        });
        let input = json!({"name": 42});

        assert!(validate(&schema, &input).is_err());
    }
}
