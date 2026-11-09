//! Checks the hand-written types against Muse's committed schema (D-022).
//!
//! In unit tests every wire type derives `schemars::JsonSchema`, so each one's member names,
//! required members and primitive types can be compared with its `$defs` entry:
//! - every member a type declares exists in the definition, with a compatible JSON type;
//! - a type this crate **decodes** requires nothing the schema leaves optional;
//! - a type this crate **sends** has every member the schema requires, and always sends it;
//! - every open enum's known values exist in the schema's enum, and cover it.
//!
//! `tests/fixtures.rs` and `tests/echo.rs` complete this: real frames from Muse validate against
//! the schema and decode into these types.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde_json::Value;

use crate::methods::METHODS;
use crate::types::*;
use crate::{ErrorKind, SCHEMA_FINGERPRINT};

const SCHEMA: &str = include_str!("../schema/msp.schema.json");
const MANIFEST: &str = include_str!("../schema/manifest.json");

fn schema() -> Value {
    serde_json::from_str(SCHEMA).expect("the committed schema is JSON")
}

fn definition<'a>(schema: &'a Value, name: &str) -> &'a Value {
    schema["$defs"]
        .get(name)
        .unwrap_or_else(|| panic!("the schema has no definition {name}"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Direction {
    /// Decoded from Muse.
    In,
    /// Sent to Muse.
    Out,
    /// Both.
    Both,
}

fn ours<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap()
}

fn names(value: Option<&Value>) -> BTreeSet<String> {
    match value {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// The JSON types a property schema allows, without `null`; `None` when it doesn't say
/// directly (a `$ref`, `anyOf`, and so on).
fn json_types(property: &Value) -> Option<BTreeSet<String>> {
    let types: BTreeSet<String> = match property.get("type")? {
        Value::String(single) => [single.clone()].into(),
        Value::Array(many) => many
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect(),
        _ => return None,
    };
    let types: BTreeSet<String> = types.into_iter().filter(|t| t != "null").collect();
    (!types.is_empty()).then_some(types)
}

fn compare(type_name: &str, ours: &Value, theirs: &Value, direction: Direction) -> Vec<String> {
    let mut problems = Vec::new();
    let our_props = names(ours.get("properties"));
    let their_props = names(theirs.get("properties"));
    let our_required = names(ours.get("required"));
    let their_required = names(theirs.get("required"));

    for unknown in our_props.difference(&their_props) {
        problems.push(format!("{type_name}.{unknown} isn't in the schema"));
    }
    for property in our_props.intersection(&their_props) {
        let (Some(a), Some(b)) = (
            json_types(&ours["properties"][property]),
            json_types(&theirs["properties"][property]),
        ) else {
            continue;
        };
        if a.is_disjoint(&b) {
            problems.push(format!(
                "{type_name}.{property} is {a:?} here but {b:?} in the schema"
            ));
        }
    }
    if matches!(direction, Direction::In | Direction::Both) {
        for required in our_required.difference(&their_required) {
            problems.push(format!(
                "{type_name}.{required} is required here but optional in the schema"
            ));
        }
    }
    if matches!(direction, Direction::Out | Direction::Both) {
        for missing in their_required.difference(&our_required) {
            problems.push(format!(
                "{type_name}.{missing} is required by the schema but optional or missing here"
            ));
        }
    }
    problems
}

macro_rules! struct_checks {
    ($($ty:ty => [$($def:literal),+], $direction:ident;)+) => {
        vec![$(
            (stringify!($ty), ours::<$ty>(), vec![$($def),+], Direction::$direction),
        )+]
    };
}

#[test]
fn structs_agree_with_their_schema_definitions() {
    let schema = schema();
    let checks = struct_checks! {
        ClientInfo => ["ClientInfo"], Both;
        ClientCapabilities => ["ClientCapabilities"], Out;
        InitializeParams => ["InitializeParams"], Out;
        InitializeResult => ["InitializeResult"], In;
        ServerInfo => ["ServerInfo"], In;
        SchemaInfo => ["SchemaInfo"], In;
        SessionStartParams => ["SessionStartParams"], Out;
        SessionStartResult => ["SessionStartResult"], In;
        Session => ["Session"], In;
        EffectiveApprovalModeState => ["EffectiveApprovalModeState"], In;
        SessionSetApprovalModeParams => ["SessionSetApprovalModeParams"], Out;
        SessionSetApprovalModeResult => ["SessionSetApprovalModeResult"], In;
        SessionStartedParams => ["SessionStartedParams"], In;
        SessionStatusChangedParams => ["SessionStatusChangedParams"], In;
        SessionClosedParams => ["SessionClosedParams"], In;
        SessionModelChangedParams => ["SessionModelChangedParams"], In;
        ViewGapParams => ["ViewGapParams"], In;
        TurnInputPart => ["TurnInputPart"], Out;
        TurnStartParams => ["TurnStartParams"], Out;
        TurnStartResult => ["TurnStartResult"], In;
        TurnInterruptParams => ["TurnInterruptParams"], Out;
        TurnCancelParams => ["TurnCancelParams"], Out;
        TurnInterruptResult => ["TurnInterruptResult", "TurnCancelResult"], In;
        TurnStartedParams => ["TurnStartedParams"], In;
        TurnForegroundCompletedParams => ["TurnForegroundCompletedParams"], In;
        TurnCompletedParams => ["TurnCompletedParams"], In;
        TurnError => ["TurnError"], In;
        QuotaDetail => ["QuotaDetail"], In;
        TokenUsage => ["TokenUsage"], In;
        Item => ["Item"], In;
        ToolDisplayLabel => ["ToolDisplayLabel"], In;
        OutputRef => ["OutputRef"], In;
        PatchSummary => ["PatchSummary"], In;
        ItemParams => ["ItemStartedParams", "ItemUpdatedParams", "ItemCompletedParams"], In;
        ItemDeltaParams => ["ItemDeltaParams"], In;
        ItemReadOutputParams => ["ItemReadOutputParams"], Out;
        ItemReadOutputResult => ["ItemReadOutputResult"], In;
        ApprovalChoice => ["ApprovalChoice"], In;
        ApprovalRequirementRef => ["ApprovalRequirementRef"], Both;
        ApprovalStageResolution => ["ApprovalStageResolution"], In;
        ApprovalSuggestedPrefix => ["ApprovalSuggestedPrefix"], In;
        ApprovalStage => ["ApprovalStage"], In;
        ApprovalSubject => ["ApprovalSubject"], In;
        ApprovalRequestParams => ["ApprovalRequestParams"], In;
        ApprovalDecideParams => ["ApprovalDecideParams"], Out;
        ApprovalDecideResult => ["ApprovalDecideResult"], In;
        ApprovalChange => ["ApprovalChange"], In;
        ApprovalUpdatedParams => ["ApprovalUpdatedParams"], In;
        ApprovalResolvedParams => ["ApprovalResolvedParams"], In;
        ApprovalListPendingParams => ["ApprovalListPendingParams"], Out;
        ApprovalListPendingResult => ["ApprovalListPendingResult"], In;
        SubscriptionUsage => ["SubscriptionUsage"], In;
        SubscriptionUsageWindow => ["SubscriptionUsageWindow"], In;
        SubscriptionUsageWeekly => ["SubscriptionUsageWeekly"], In;
        UsageReadResult => ["UsageReadResult"], In;
        CumulativeTokenUsage => ["CumulativeTokenUsage"], In;
        SessionTokenUsageParams => ["SessionTokenUsageParams"], In;
        SessionContextUsageParams => ["SessionContextUsageParams"], In;
        ModelListParams => ["ModelListParams"], Out;
        ModelCost => ["ModelCost"], In;
        ModelCatalogEntry => ["ModelCatalogEntry"], In;
        ModelListResult => ["ModelListResult"], In;
        ModelSelection => ["ModelSelection"], Out;
        SessionSetModelParams => ["SessionSetModelParams"], Out;
        SessionSetModelResult => ["SessionSetModelResult"], In;
        UserInputOption => ["UserInputOption"], In;
        UserInputSelection => ["UserInputSelection"], In;
        UserInputQuestion => ["UserInputQuestion"], In;
        UserInputRequestParams => ["UserInputRequestParams"], In;
        UserInputAnswer => ["UserInputAnswer"], Both;
        UserInputAnswerParams => ["UserInputAnswerParams"], Out;
        UserInputAnswerResult => ["UserInputAnswerResult", "UserInputCancelResult"], In;
        UserInputCancelParams => ["UserInputCancelParams"], Out;
        UserInputSettledParams => ["UserInputSettledParams"], In;
    };
    let mut problems = Vec::new();
    for (type_name, our_schema, definitions, direction) in &checks {
        for name in definitions {
            problems.extend(compare(
                type_name,
                our_schema,
                definition(&schema, name),
                *direction,
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The values a schema definition's enum lists.
fn enum_values(definition: &Value) -> BTreeSet<String> {
    if let Some(values) = definition.get("enum") {
        return names(Some(values));
    }
    let mut values = BTreeSet::new();
    for branch in ["oneOf", "anyOf"] {
        for option in definition
            .get(branch)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(value) = option.get("const").and_then(Value::as_str) {
                values.insert(value.to_owned());
            }
            values.extend(names(option.get("enum")));
        }
    }
    values
}

#[test]
fn open_enums_know_exactly_the_schema_values() {
    let schema = schema();
    let checks: &[(&str, &[&str])] = &[
        ("ReasoningEffort", ReasoningEffort::KNOWN),
        ("ApprovalMode", ApprovalMode::KNOWN),
        ("SessionStatus", SessionStatus::KNOWN),
        ("AttentionFlag", AttentionFlag::KNOWN),
        ("SessionClosedReason", SessionClosedReason::KNOWN),
        ("SandboxAvailability", SandboxAvailability::KNOWN),
        ("SessionDurability", SessionDurability::KNOWN),
        ("TurnStartDisposition", TurnStartDisposition::KNOWN),
        ("TurnTerminal", TurnTerminal::KNOWN),
        ("TurnErrorKind", TurnErrorKind::KNOWN),
        ("LimitKind", LimitKind::KNOWN),
        ("ItemKind", ItemKind::KNOWN),
        ("ItemStatus", ItemStatus::KNOWN),
        ("MessagePhase", MessagePhase::KNOWN),
        ("ApprovalDecision", ApprovalDecision::KNOWN),
        ("ApprovalChoiceScope", ApprovalChoiceScope::KNOWN),
        ("ApprovalPolicyResult", ApprovalPolicyResult::KNOWN),
        ("ApprovalResolvedBy", ApprovalResolvedBy::KNOWN),
        ("ContextPressureLevel", ContextPressure::KNOWN),
    ];
    let mut problems = Vec::new();
    for (name, known) in checks {
        let theirs = enum_values(definition(&schema, name));
        let ours: BTreeSet<String> = known.iter().map(|v| (*v).to_owned()).collect();
        if ours != theirs {
            problems.push(format!(
                "{name}: only here {:?}, only in the schema {:?}",
                ours.difference(&theirs).collect::<Vec<_>>(),
                theirs.difference(&ours).collect::<Vec<_>>(),
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn closed_enums_match_the_schema() {
    let schema = schema();
    let checks = [
        ("TurnInputPartType", ours::<TurnInputPartType>()),
        ("IfBusy", ours::<IfBusy>()),
        ("ItemReadOutputEncoding", ours::<OutputEncoding>()),
    ];
    for (name, our_schema) in checks {
        assert_eq!(
            enum_values(&our_schema),
            enum_values(definition(&schema, name)),
            "{name}"
        );
    }
}

#[test]
fn error_kinds_are_the_error_table() {
    let schema = schema();
    let mut table = BTreeSet::new();
    for error in schema["errors"].as_array().unwrap() {
        table.insert(error["kind"].as_str().unwrap().to_owned());
        table.extend(names(error.get("overrideKinds")));
        let code = error["code"].as_i64().unwrap();
        assert_eq!(
            ErrorKind::from_code(code).as_str(),
            error["kind"].as_str().unwrap(),
            "code {code}"
        );
    }
    for known in ErrorKind::KNOWN {
        assert!(table.contains(*known), "{known} isn't in the error table");
    }
}

#[test]
fn every_method_marker_is_a_schema_method() {
    let schema = schema();
    for name in METHODS {
        assert!(
            schema["methods"].get(*name).is_some(),
            "{name} isn't in the schema's methods"
        );
    }
    // `usage/read` takes no params, which is why its marker sends none.
    assert!(schema["methods"]["usage/read"].get("params").is_none());
}

#[test]
fn the_fingerprint_constant_is_the_committed_manifest() {
    let manifest: Value = serde_json::from_str(MANIFEST).unwrap();
    assert_eq!(manifest["fingerprint"], SCHEMA_FINGERPRINT);
    assert_eq!(manifest["experimental"], false);
}
