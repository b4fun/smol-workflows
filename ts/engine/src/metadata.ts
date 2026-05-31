import { readFile } from "node:fs/promises";
import { parse } from "acorn-loose";
import type { Node, Program } from "acorn";
import type { WorkflowMetadata, WorkflowPhaseMetadata } from "@smol-workflow/sdk";

export async function readWorkflowMetadata(
  scriptPath: string,
): Promise<WorkflowMetadata | undefined> {
  const source = await readFile(scriptPath, "utf8").catch(() => undefined);

  if (!source) {
    return undefined;
  }

  return extractWorkflowMetadata(source);
}

/**
 * Statically extracts `export const meta = {...}` before importing a workflow.
 *
 * This extra parse round is necessary for the SDK's preferred top-level ESM
 * workflow style: importing the module evaluates top-level `phase()` and
 * `agent()` calls before `module.meta` is available to the runner. Phase-level
 * provider/model defaults must therefore be known before `import()` starts.
 *
 * The workflow reference requires `meta` to be a pure literal. We enforce that
 * here by parsing with acorn-loose and converting only literal object/array
 * syntax into data, rather than evaluating arbitrary JavaScript.
 */
export function extractWorkflowMetadata(source: string): WorkflowMetadata | undefined {
  const program = parse(source, {
    ecmaVersion: "latest",
    sourceType: "module",
  }) as Program;
  const metaInitializer = findExportedConstInitializer(program, "meta");

  if (!metaInitializer) {
    return undefined;
  }

  const value = readPureLiteral(metaInitializer);

  if (value === invalidLiteral) {
    return undefined;
  }

  return toWorkflowMetadata(value);
}

function findExportedConstInitializer(program: Program, name: string): Node | undefined {
  for (const statement of program.body) {
    const node = asNodeRecord(statement);

    if (node.type !== "ExportNamedDeclaration") {
      continue;
    }

    const declaration = node.declaration;

    if (!isNodeRecord(declaration) || declaration.type !== "VariableDeclaration") {
      continue;
    }

    if (declaration.kind !== "const" || !Array.isArray(declaration.declarations)) {
      continue;
    }

    for (const variable of declaration.declarations) {
      if (!isNodeRecord(variable) || variable.type !== "VariableDeclarator") {
        continue;
      }

      const id = variable.id;

      if (isNodeRecord(id) && id.type === "Identifier" && id.name === name && isNode(variable.init)) {
        return variable.init;
      }
    }
  }

  return undefined;
}

const invalidLiteral = Symbol("invalid workflow metadata literal");

type LiteralReadResult = unknown | typeof invalidLiteral;

function readPureLiteral(node: Node): LiteralReadResult {
  const record = asNodeRecord(node);

  switch (record.type) {
    case "ObjectExpression":
      return readObjectLiteral(record);
    case "ArrayExpression":
      return readArrayLiteral(record);
    case "Literal":
      return readPrimitiveLiteral(record);
    case "UnaryExpression":
      return readUnaryLiteral(record);
    default:
      return invalidLiteral;
  }
}

function readObjectLiteral(node: Record<string, unknown>): LiteralReadResult {
  if (!Array.isArray(node.properties)) {
    return invalidLiteral;
  }

  const object: Record<string, unknown> = {};

  for (const property of node.properties) {
    if (!isNodeRecord(property) || property.type !== "Property" || property.computed) {
      return invalidLiteral;
    }

    const key = readPropertyKey(property.key);

    if (key === undefined || !isNode(property.value)) {
      return invalidLiteral;
    }

    const value = readPureLiteral(property.value);

    if (value === invalidLiteral) {
      return invalidLiteral;
    }

    object[key] = value;
  }

  return object;
}

function readArrayLiteral(node: Record<string, unknown>): LiteralReadResult {
  if (!Array.isArray(node.elements)) {
    return invalidLiteral;
  }

  const array: unknown[] = [];

  for (const element of node.elements) {
    if (!isNode(element)) {
      return invalidLiteral;
    }

    const value = readPureLiteral(element);

    if (value === invalidLiteral) {
      return invalidLiteral;
    }

    array.push(value);
  }

  return array;
}

function readPropertyKey(key: unknown): string | undefined {
  if (!isNodeRecord(key)) {
    return undefined;
  }

  if (key.type === "Identifier" && typeof key.name === "string") {
    return key.name;
  }

  if (key.type === "Literal") {
    const value = readPrimitiveLiteral(key);
    return typeof value === "string" || typeof value === "number" ? String(value) : undefined;
  }

  return undefined;
}

function readPrimitiveLiteral(node: Record<string, unknown>): LiteralReadResult {
  const value = node.value;

  return typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean" ||
    value === null
    ? value
    : invalidLiteral;
}

function readUnaryLiteral(node: Record<string, unknown>): LiteralReadResult {
  if ((node.operator !== "-" && node.operator !== "+") || !isNode(node.argument)) {
    return invalidLiteral;
  }

  const value = readPureLiteral(node.argument);

  if (typeof value !== "number") {
    return invalidLiteral;
  }

  return node.operator === "-" ? -value : value;
}

type NodeRecord = Node & Record<string, unknown>;

function isNode(value: unknown): value is Node {
  return isRecord(value) && typeof value.type === "string";
}

function isNodeRecord(value: unknown): value is NodeRecord {
  return isNode(value);
}

function asNodeRecord(node: Node): NodeRecord {
  return node as NodeRecord;
}

export function toWorkflowMetadata(value: unknown): WorkflowMetadata | undefined {
  if (!isRecord(value) || typeof value.name !== "string" || typeof value.description !== "string") {
    return undefined;
  }

  const phases = Array.isArray(value.phases)
    ? value.phases.map(toWorkflowPhaseMetadata).filter((phase) => phase !== undefined)
    : undefined;

  return {
    name: value.name,
    description: value.description,
    ...(typeof value.whenToUse === "string" ? { whenToUse: value.whenToUse } : {}),
    ...(phases ? { phases } : {}),
  };
}

function toWorkflowPhaseMetadata(value: unknown): WorkflowPhaseMetadata | undefined {
  if (!isRecord(value) || typeof value.title !== "string") {
    return undefined;
  }

  return {
    title: value.title,
    ...(typeof value.detail === "string" ? { detail: value.detail } : {}),
    ...(typeof value.model === "string" ? { model: value.model } : {}),
    ...(typeof value.provider === "string" ? { provider: value.provider } : {}),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
