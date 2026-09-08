import fs from "node:fs";
import path from "node:path";
import assert from "node:assert/strict";
import ts from "typescript";

const allowed = {
  "main.tsx": ["App.tsx", "styles.css", "react-dom/client"],
  "App.tsx": ["i18n.ts", "usePrintWorkflow.ts", "contracts.ts", "react", "@heroui/react"],
  "usePrintWorkflow.ts": ["i18n.ts", "native.ts", "contracts.ts", "react"],
  "native.ts": ["i18n.ts", "contracts.ts", "@tauri-apps/api/core", "@tauri-apps/api/webview", "@tauri-apps/plugin-dialog"],
  "i18n.ts": ["contracts.ts", "locales/en.json", "locales/ko.json"],
  "locales/en.json": [],
  "locales/ko.json": [],
  "contracts.ts": [],
  "styles.css": ["design-tokens.css", "tailwindcss", "@heroui/styles"],
  "design-tokens.css": [],
};
function cycleCheck(graph) {
  const done = new Set(), stack = [];
  function visit(name) {
    if (stack.includes(name)) throw new Error(`cycle: ${[...stack.slice(stack.indexOf(name)), name].join(" -> ")}`);
    if (done.has(name)) return;
    stack.push(name);
    for (const edge of graph[name] ?? []) if (edge in graph) visit(edge);
    stack.pop(); done.add(name);
  }
  Object.keys(graph).forEach(visit);
}
function check(sources) {
  const graph = {};
  for (const [name, source] of Object.entries(sources)) {
    assert.ok(name in allowed, `unclassified production file: ${name}`);
    const modules = [];
    if (name.endsWith(".css")) {
      const clean = source.replace(/\/\*[\s\S]*?\*\//g, "");
      const imports = [...clean.matchAll(/@import\s+(["'])([^"']+)\1\s*;/g)];
      assert.equal(imports.length, (clean.match(/@import\b/g) ?? []).length, `unsupported CSS import: ${name}`);
      modules.push(...imports.map(match => match[2]));
    } else if (name.endsWith(".json")) {
      JSON.parse(source);
    } else {
      const file = ts.createSourceFile(name, source, ts.ScriptTarget.Latest, true);
      assert.equal(file.parseDiagnostics.length, 0, `parse error: ${name}`);
      function literal(node) {
        assert.ok(node && ts.isStringLiteralLike(node), `nonliteral module load: ${name}`);
        modules.push(node.text);
      }
      function visit(node) {
        if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier) literal(node.moduleSpecifier);
        if (ts.isImportEqualsDeclaration(node) && ts.isExternalModuleReference(node.moduleReference)) literal(node.moduleReference.expression);
        if (ts.isImportTypeNode(node)) literal(ts.isLiteralTypeNode(node.argument) ? node.argument.literal : undefined);
        if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(node.expression) && node.expression.text === "require"))) literal(node.arguments[0]);
        ts.forEachChild(node, visit);
      }
      visit(file);
    }
    graph[name] = [...new Set(modules.map(specifier => {
      if (!specifier.startsWith(".")) return specifier;
      const base = path.posix.normalize(path.posix.join(path.posix.dirname(name), specifier));
      const resolved = [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`].filter(candidate => candidate in sources);
      assert.equal(resolved.length, 1, `unresolved/ambiguous module: ${name} -> ${specifier}`);
      return resolved[0];
    }))];
  }
  cycleCheck(graph);
  for (const [name, edges] of Object.entries(graph)) for (const edge of edges) assert.ok(allowed[name].includes(edge), `forbidden edge: ${name} -> ${edge}`);
  return graph;
}
const sample = Object.fromEntries(Object.keys(allowed).map(name => [name, name.endsWith(".json") ? "{}" : ""]));
check({ ...sample, "main.tsx": 'import App from "./App"; import "./styles.css"; import { createRoot } from "react-dom/client";' });
for (const [name, code] of [
  ["i18n.ts", 'import "./native";'],
  ["contracts.ts", 'import type { X } from "./App";'],
  ["native.ts", 'export * from "./usePrintWorkflow";'],
  ["contracts.ts", 'type X = import("./native").X;'],
  ["App.tsx", 'const x = import("@tauri-apps/api/core");'],
  ["App.tsx", 'const x = require("@tauri-apps/api/core");'],
  ["App.tsx", 'const x = import(variable);'],
  ["main.tsx", 'import "@tauri-apps/api/core";'],
  ["main.tsx", 'import "@heroui/react";'],
  ["App.tsx", 'import "react-dom/client";'],
]) assert.throws(() => check({ ...sample, [name]: code }));
assert.throws(() => check({ ...sample, "new.ts": "" }), /unclassified/);
for (const name of ["App.tsx", "usePrintWorkflow.ts", "native.ts", "contracts.ts"]) assert.throws(() => check({ ...sample, [name]: 'import "react-dom/client";' }), /forbidden edge/);
assert.throws(() => check({ ...sample, "App.tsx": 'import "./usePrintWorkflow";', "usePrintWorkflow.ts": 'import "./App";' }), /cycle: App.tsx -> usePrintWorkflow.ts -> App.tsx/);
const root = new URL("../src/", import.meta.url);
const config = JSON.parse(fs.readFileSync(new URL("../tsconfig.json", import.meta.url), "utf8"));
assert.ok(!config.extends && !config.compilerOptions?.paths && !config.compilerOptions?.baseUrl, "unreviewed TypeScript alias configuration");
const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"));
assert.ok(!pkg.imports && !pkg.exports, "unreviewed package module mappings");
assert.ok(Object.entries({ ...pkg.dependencies, ...pkg.devDependencies }).every(([name, version]) => !version.startsWith("npm:") && !(name in allowed)), "unreviewed package alias");
const vite = ts.createSourceFile("vite.config.ts", fs.readFileSync(new URL("../vite.config.ts", import.meta.url), "utf8"), ts.ScriptTarget.Latest, true);
function checkConfig(node) {
  if (ts.isPropertyAssignment(node)) assert.ok(!["alias", "resolve"].includes(node.name.getText(vite).replace(/["']/g, "")), "unreviewed Vite resolve/alias configuration");
  ts.forEachChild(node, checkConfig);
}
checkConfig(vite);
const sources = {};
function collect(dir, prefix = "") {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const name = prefix + entry.name, file = new URL(entry.name, dir);
    if (entry.isDirectory()) collect(new URL(`${entry.name}/`, dir), `${name}/`);
    else if (/\.(tsx?|css|json)$/.test(name) && !/\.test\.[^/]+$/.test(name)) sources[name] = fs.readFileSync(file, "utf8");
  }
}
collect(root);
const graph = check(sources);
console.log("TypeScript/CSS architecture: self-tests PASS; production cycles 0");
for (const [name, edges] of Object.entries(graph)) console.log(`${name} -> ${edges.join(", ") || "(leaf)"}`);
