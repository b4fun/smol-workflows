import { spawn, type ChildProcessByStdio } from "node:child_process";
import { join } from "node:path";
import type { Readable } from "node:stream";

export function fixturePath(name: string): string {
  return join(process.cwd(), "test", "fixtures", name);
}

export function spawnWorkflowCli(args: string[]): ChildProcessByStdio<null, Readable, Readable> {
  return spawn(process.execPath, ["dist/cli.js", ...args], {
    cwd: process.cwd(),
    stdio: ["ignore", "pipe", "pipe"],
  });
}

export function spawnWorkflowRun(args: string[]): ChildProcessByStdio<null, Readable, Readable> {
  return spawnWorkflowCli(["run", ...args]);
}

export function collectProcess(
  child: ChildProcessByStdio<null, Readable, Readable>,
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  let stdout = "";
  let stderr = "";

  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");

  child.stdout.on("data", (chunk: string) => {
    stdout += chunk;
  });

  child.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });

  return new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => {
      resolve({ code, stdout, stderr });
    });
  });
}
