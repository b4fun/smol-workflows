/** A primitive JSON value. */
export type JSONPrimitive = string | number | boolean | null;

/** Any value that can be represented in JSON. */
export type JSONValue = JSONPrimitive | JSONObject | JSONArray;

/** A JSON object with string keys and JSON-compatible values. */
export type JSONObject = { [key: string]: JSONValue };

/** A JSON array containing JSON-compatible values. */
export type JSONArray = JSONValue[];

/** Valid values for the JSON Schema `type` keyword. */
export type JSONSchemaType =
  | "null"
  | "boolean"
  | "object"
  | "array"
  | "number"
  | "integer"
  | "string";

/**
 * A JSON Schema document or subschema.
 *
 * Boolean schemas are supported: `true` accepts anything, `false` rejects everything.
 */
export type JSONSchema = boolean | JSONSchemaObject;

/**
 * A JSON Schema object using common JSON Schema keywords.
 *
 * This is intended for SDK typing and editor help, not as a complete validator.
 */
export type JSONSchemaObject = {
  $id?: string;
  $schema?: string;
  $ref?: string;
  $defs?: Record<string, JSONSchema>;
  definitions?: Record<string, JSONSchema>;

  title?: string;
  description?: string;
  default?: JSONValue;
  examples?: readonly JSONValue[];

  type?: JSONSchemaType | readonly JSONSchemaType[];
  enum?: readonly JSONValue[];
  const?: JSONValue;

  properties?: Record<string, JSONSchema>;
  patternProperties?: Record<string, JSONSchema>;
  additionalProperties?: JSONSchema;
  required?: readonly string[];
  propertyNames?: JSONSchema;
  minProperties?: number;
  maxProperties?: number;

  items?: JSONSchema | readonly JSONSchema[];
  prefixItems?: readonly JSONSchema[];
  additionalItems?: JSONSchema;
  minItems?: number;
  maxItems?: number;
  uniqueItems?: boolean;

  minLength?: number;
  maxLength?: number;
  pattern?: string;
  format?: string;

  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  multipleOf?: number;

  allOf?: readonly JSONSchema[];
  anyOf?: readonly JSONSchema[];
  oneOf?: readonly JSONSchema[];
  not?: JSONSchema;
  if?: JSONSchema;
  then?: JSONSchema;
  else?: JSONSchema;
  dependentSchemas?: Record<string, JSONSchema>;
};
