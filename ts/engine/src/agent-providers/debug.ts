import type { JSONSchema, JSONSchemaObject, JSONSchemaType, JSONValue } from "@smol-workflow/sdk";
import type { AgentProvider, AgentProviderResult, AgentProviderRunInput } from "./types.js";

export function createDebugAgentProvider(): AgentProvider {
  return {
    name: "debug",
    schemaMode: "builtin",
    usageMode: "builtin",
    async run(input: AgentProviderRunInput): Promise<AgentProviderResult> {
      const output = input.options?.schema
        ? generateDebugValueFromSchema(input.options.schema)
        : `echo: ${input.prompt}`;

      return {
        output,
        usage: {
          inputTokens: estimateTokens(input.prompt),
          outputTokens: estimateTokens(JSON.stringify(output)),
          totalTokens: estimateTokens(input.prompt) + estimateTokens(JSON.stringify(output)),
          cost: {
            input: 0,
            output: 0,
            total: 0,
            currency: "USD",
          },
        },
        raw: toJSONValue({ output }),
      };
    },
  };
}

export function generateDebugValueFromSchema(schema: JSONSchema): JSONValue {
  if (schema === true) {
    return "debug";
  }

  if (schema === false) {
    return null;
  }

  if (schema.const !== undefined) {
    return schema.const;
  }

  if (schema.enum && schema.enum.length > 0) {
    return schema.enum[0] ?? null;
  }

  if (schema.oneOf && schema.oneOf.length > 0) {
    return generateDebugValueFromSchema(schema.oneOf[0] ?? true);
  }

  if (schema.anyOf && schema.anyOf.length > 0) {
    return generateDebugValueFromSchema(schema.anyOf[0] ?? true);
  }

  if (schema.allOf && schema.allOf.length > 0) {
    return mergeAllOf(schema.allOf);
  }

  const type = firstSchemaType(schema.type) ?? inferSchemaType(schema);

  switch (type) {
    case "null":
      return null;
    case "boolean":
      return true;
    case "integer":
      return 0;
    case "number":
      return 0;
    case "string":
      return debugString(schema);
    case "array":
      return debugArray(schema);
    case "object":
      return debugObject(schema);
  }
}

function mergeAllOf(schemas: readonly JSONSchema[]): JSONValue {
  const values = schemas.map((schema) => generateDebugValueFromSchema(schema));

  if (values.every(isJSONObject)) {
    return Object.assign({}, ...values) as JSONValue;
  }

  return values[values.length - 1] ?? null;
}

function firstSchemaType(type: JSONSchemaObject["type"]): JSONSchemaType | undefined {
  return Array.isArray(type) ? type[0] : (type as JSONSchemaType | undefined);
}

function inferSchemaType(schema: JSONSchemaObject): JSONSchemaType {
  if (schema.properties || schema.required || schema.additionalProperties !== undefined) {
    return "object";
  }

  if (schema.items || schema.prefixItems) {
    return "array";
  }

  if (schema.minimum !== undefined || schema.maximum !== undefined || schema.multipleOf !== undefined) {
    return "number";
  }

  if (schema.minLength !== undefined || schema.maxLength !== undefined || schema.pattern || schema.format) {
    return "string";
  }

  return "object";
}

function debugString(schema: JSONSchemaObject): string {
  if (schema.format === "email") {
    return "debug@example.com";
  }

  if (schema.format === "uri" || schema.format === "url") {
    return "https://example.com/debug";
  }

  if (schema.format === "date-time") {
    return "2000-01-01T00:00:00.000Z";
  }

  if (schema.format === "date") {
    return "2000-01-01";
  }

  return "debug-string";
}

function debugArray(schema: JSONSchemaObject): JSONValue[] {
  if (Array.isArray(schema.prefixItems) && schema.prefixItems.length > 0) {
    return schema.prefixItems.map((item) => generateDebugValueFromSchema(item));
  }

  const items = schema.items;

  if (items && !isSchemaTuple(items)) {
    return [generateDebugValueFromSchema(items)];
  }

  return [];
}

function debugObject(schema: JSONSchemaObject): { [key: string]: JSONValue } {
  const output: { [key: string]: JSONValue } = {};
  const properties = schema.properties ?? {};
  const keys = new Set([...Object.keys(properties), ...(schema.required ?? [])]);

  for (const key of keys) {
    output[key] = generateDebugValueFromSchema(properties[key] ?? true);
  }

  return output;
}

function isSchemaTuple(value: unknown): value is readonly JSONSchema[] {
  return Array.isArray(value);
}

function estimateTokens(text: string): number {
  return Math.max(1, Math.ceil(text.length / 4));
}

function toJSONValue(value: unknown): JSONValue {
  return JSON.parse(JSON.stringify(value)) as JSONValue;
}

function isJSONObject(value: JSONValue): value is { [key: string]: JSONValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
