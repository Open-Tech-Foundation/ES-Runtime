// Editions / proto3 feature model. We resolve three features that affect the
// wire codec: field presence, repeated encoding, and enum openness. Others
// (utf8_validation, message_encoding, json_format) don't change binary decode.

export type FieldPresence = "EXPLICIT" | "IMPLICIT" | "LEGACY_REQUIRED";
export type RepeatedEncoding = "PACKED" | "EXPANDED";
export type EnumKind = "OPEN" | "CLOSED";

export interface FeatureSet {
  fieldPresence?: FieldPresence;
  repeatedEncoding?: RepeatedEncoding;
  enumType?: EnumKind;
}

/** Baseline feature set for a syntax/edition. proto3 and edition 2023 differ
 *  only in default field presence (proto3 implicit, 2023 explicit). */
export function baseFeatures(syntax: "proto3" | "2023"): Required<FeatureSet> {
  if (syntax === "proto3") {
    return { fieldPresence: "IMPLICIT", repeatedEncoding: "PACKED", enumType: "OPEN" };
  }
  // edition 2023 defaults
  return { fieldPresence: "EXPLICIT", repeatedEncoding: "PACKED", enumType: "OPEN" };
}

/** Merges an override set over an inherited set (override wins per key). */
export function mergeFeatures(base: Required<FeatureSet>, over: FeatureSet | undefined): Required<FeatureSet> {
  if (!over) return base;
  return {
    fieldPresence: over.fieldPresence ?? base.fieldPresence,
    repeatedEncoding: over.repeatedEncoding ?? base.repeatedEncoding,
    enumType: over.enumType ?? base.enumType,
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
    default:
      return null;
  }
}
