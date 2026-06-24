// Editions / proto3 feature model. We resolve four features that affect the
// wire codec: field presence, repeated encoding, enum openness, and message
// encoding (length-prefixed vs delimited/group). Others (utf8_validation,
// json_format) don't change binary decode.

export type FieldPresence = "EXPLICIT" | "IMPLICIT" | "LEGACY_REQUIRED";
export type RepeatedEncoding = "PACKED" | "EXPANDED";
export type EnumKind = "OPEN" | "CLOSED";
export type MessageEncoding = "LENGTH_PREFIXED" | "DELIMITED";

export interface FeatureSet {
  fieldPresence?: FieldPresence;
  repeatedEncoding?: RepeatedEncoding;
  enumType?: EnumKind;
  messageEncoding?: MessageEncoding;
}

/** Baseline feature set for a syntax/edition. proto3 differs from editions only
 *  in default field presence (proto3 implicit, editions explicit). Editions 2023
 *  and 2024 share the same defaults for the four wire-affecting features. */
export function baseFeatures(syntax: "proto3" | "2023" | "2024"): Required<FeatureSet> {
  if (syntax === "proto3") {
    return { fieldPresence: "IMPLICIT", repeatedEncoding: "PACKED", enumType: "OPEN", messageEncoding: "LENGTH_PREFIXED" };
  }
  // edition 2023 / 2024 defaults
  return { fieldPresence: "EXPLICIT", repeatedEncoding: "PACKED", enumType: "OPEN", messageEncoding: "LENGTH_PREFIXED" };
}

/** Merges an override set over an inherited set (override wins per key). */
export function mergeFeatures(base: Required<FeatureSet>, over: FeatureSet | undefined): Required<FeatureSet> {
  if (!over) return base;
  return {
    fieldPresence: over.fieldPresence ?? base.fieldPresence,
    repeatedEncoding: over.repeatedEncoding ?? base.repeatedEncoding,
    enumType: over.enumType ?? base.enumType,
    messageEncoding: over.messageEncoding ?? base.messageEncoding,
  };
}

/** Parses a `features.*` option key/value into a partial FeatureSet. Returns
 *  null if the key isn't a feature we track. */
export function featureFromOption(key: string, value: string): FeatureSet | null {
  switch (key) {
    case "features.field_presence":
      return { fieldPresence: value as FieldPresence };
    case "features.repeated_field_encoding":
      return { repeatedEncoding: value as RepeatedEncoding };
    case "features.enum_type":
      return { enumType: value as EnumKind };
    case "features.message_encoding":
      return { messageEncoding: value as MessageEncoding };
    default:
      return null;
  }
}
