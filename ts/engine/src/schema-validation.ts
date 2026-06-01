import { Ajv, type ErrorObject } from "ajv";

const ajv = new Ajv({ allErrors: true, strict: false });

export type SchemaValidationResult =
  | { valid: true }
  | { valid: false; errors: string[] };

export function validateStructuredOutput(schema: unknown, output: unknown): SchemaValidationResult {
  const validate = ajv.compile(schema as Parameters<typeof ajv.compile>[0]);

  if (validate(output)) {
    return { valid: true };
  }

  return {
    valid: false,
    errors: formatValidationErrors(validate.errors ?? []),
  };
}

export function formatStructuredOutputValidationError(errors: readonly string[]): string {
  return `Structured output did not match JSON Schema: ${errors.join("; ")}`;
}

export function withStructuredOutputRetryPrompt(prompt: string, errors: readonly string[]): string {
  return [
    prompt,
    "",
    "Previous structured output failed JSON Schema validation.",
    "Return a corrected structured output that satisfies the original JSON Schema.",
    "Validation errors:",
    ...errors.map((error) => `- ${error}`),
  ].join("\n");
}

function formatValidationErrors(errors: readonly ErrorObject[]): string[] {
  return errors.map((error) => {
    const path = error.instancePath || "/";
    return `${path} ${error.message ?? "is invalid"}`;
  });
}
