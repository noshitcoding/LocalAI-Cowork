#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const DEFAULT_DOCS_DIR = "docs/product";
const DEFAULT_APP_ROUTES_FILE = "app/src/App.tsx";
const DEFAULT_ROUTE_REGISTRY_FILE = "app/src/product/routeRegistry.ts";
const INDEX_FILE = "INDEX.md";
const CATALOG_DIR = "catalog";
const GENERATED_DIR = "generated";
const SCHEMA_DIR = "schema";
const CATALOG_INDEX_FILE = "catalog.md";

const CATALOG_KIND_CONFIG = {
  views: { kind: "view", wrapper: "views", schema: "view.schema.json" },
  elements: { kind: "element", wrapper: "elements", schema: "element.schema.json" },
  buttons: { kind: "button", wrapper: "buttons", schema: "button.schema.json" },
  infos: { kind: "info", wrapper: "infos", schema: "info.schema.json" },
  flows: { kind: "flow", wrapper: "flows", schema: "flow.schema.json" },
  domain: { kind: "domain", wrapper: "domain", schema: "domain.schema.json" },
  "design-system": { kind: "design-system", wrapper: "design_system", schema: "design-system.schema.json" },
};

const TYPE_ALIASES = new Map([
  ["route", "route"],
  ["page", "route"],
  ["screen", "route"],
  ["view", "route"],
  ["element", "element"],
  ["component", "element"],
  ["control", "element"],
  ["widget", "element"],
  ["flow", "flow"],
  ["journey", "flow"],
  ["api", "api"],
  ["endpoint", "api"],
  ["feature", "feature"],
  ["current-state", "current-state"],
  ["decision-record", "decision"],
  ["compatibility", "compatibility"],
  ["source-of-truth-index", "overview"],
  ["overview", "overview"],
  ["decision", "overview"],
  ["concept", "overview"],
]);

const ELEMENT_FIELDS = ["element", "element_id", "elementId", "selector", "id", "name"];
const ELEMENT_COLLECTION_FIELDS = ["elements", "ui_elements", "controls", "components"];

let args;
try {
  args = parseArgs(process.argv.slice(2));
} catch (error) {
  console.error(error.message);
  printUsage();
  process.exit(1);
}

if (args.help) {
  printUsage();
  process.exit(0);
}

const root = process.cwd();
const docsDir = path.resolve(root, args.docsDir || DEFAULT_DOCS_DIR);
const appRoutesFile = path.resolve(root, args.appRoutes || DEFAULT_APP_ROUTES_FILE);
const routeRegistryFile = path.resolve(root, args.routeRegistry || DEFAULT_ROUTE_REGISTRY_FILE);
const indexPath = path.join(docsDir, INDEX_FILE);
const catalogDir = path.join(docsDir, CATALOG_DIR);
const generatedDir = path.join(docsDir, GENERATED_DIR);
const schemaDir = path.join(docsDir, SCHEMA_DIR);
const catalogIndexPath = path.join(generatedDir, CATALOG_INDEX_FILE);

fs.mkdirSync(docsDir, { recursive: true });
fs.mkdirSync(generatedDir, { recursive: true });

const schemaErrors = [];
const markdownFiles = listMarkdownFiles(docsDir)
  .filter((filePath) => path.basename(filePath).toLowerCase() !== INDEX_FILE.toLowerCase())
  .filter((filePath) => !toPosix(path.relative(docsDir, filePath)).startsWith(`${GENERATED_DIR}/`))
  .sort(comparePaths);

const docs = [];

for (const filePath of markdownFiles) {
  const relPath = toPosix(path.relative(root, filePath));
  const text = fs.readFileSync(filePath, "utf8");
  const frontmatter = extractFrontmatter(text);

  if (!frontmatter) {
    schemaErrors.push(`${relPath}: missing frontmatter block`);
    continue;
  }

  const parsed = parseFrontmatter(frontmatter.raw);
  for (const error of parsed.errors) {
    schemaErrors.push(`${relPath}: ${error}`);
  }

  const doc = buildProductDoc(filePath, parsed.data);
  docs.push(doc);
  schemaErrors.push(...validateProductDoc(doc));
}

const routeRegistry = readRouteRegistry(routeRegistryFile);
schemaErrors.push(...routeRegistry.errors);
const appRoutes = mergeRoutes(readAppRoutes(appRoutesFile), routeRegistry.routes.map((route) => route.path));
const catalog = readProductCatalog({ catalogDir, schemaDir, appRoutes });
schemaErrors.push(...catalog.errors);
schemaErrors.push(...validateRouteRegistryCatalog(routeRegistry, catalog));

const indexMarkdown = generateIndex({ docs, appRoutes, schemaErrors, docsDir });
const catalogMarkdown = generateCatalogMarkdown(catalog);

if (args.check) {
  const current = fs.existsSync(indexPath) ? fs.readFileSync(indexPath, "utf8") : "";
  if (current !== indexMarkdown) {
    console.error(`${toPosix(path.relative(root, indexPath))} is out of date.`);
    console.error("Run: node scripts/validate-product-docs.mjs");
    process.exit(1);
  }

  const currentCatalog = fs.existsSync(catalogIndexPath) ? fs.readFileSync(catalogIndexPath, "utf8") : "";
  if (currentCatalog !== catalogMarkdown) {
    console.error(`${toPosix(path.relative(root, catalogIndexPath))} is out of date.`);
    console.error("Run: node scripts/validate-product-docs.mjs");
    process.exit(1);
  }
} else {
  fs.writeFileSync(indexPath, indexMarkdown, "utf8");
  console.log(`Wrote ${toPosix(path.relative(root, indexPath))}`);
  fs.writeFileSync(catalogIndexPath, catalogMarkdown, "utf8");
  console.log(`Wrote ${toPosix(path.relative(root, catalogIndexPath))}`);
}

if (schemaErrors.length > 0) {
  console.error("Product doc schema errors:");
  for (const error of schemaErrors) console.error(`- ${error}`);
  process.exit(1);
}

console.log(`Validated ${docs.length} product doc(s).`);
console.log(`Validated ${catalog.records.length} product catalog record(s).`);
console.log("Product docs validation passed.");

function parseArgs(argv) {
  const parsed = {
    appRoutes: DEFAULT_APP_ROUTES_FILE,
    check: false,
    docsDir: DEFAULT_DOCS_DIR,
    help: false,
    routeRegistry: DEFAULT_ROUTE_REGISTRY_FILE,
    routeRegistryCheck: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help" || arg === "-h") {
      parsed.help = true;
    } else if (arg === "--check") {
      parsed.check = true;
    } else if (arg === "check") {
      parsed.check = true;
    } else if (arg === "route-registry") {
      parsed.routeRegistryCheck = true;
    } else if (arg === "--docs-dir") {
      parsed.docsDir = requireValue(argv, index, arg);
      index += 1;
    } else if (arg === "--app-routes") {
      parsed.appRoutes = requireValue(argv, index, arg);
      index += 1;
    } else if (arg === "--route-registry") {
      parsed.routeRegistry = requireValue(argv, index, arg);
      index += 1;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }

  return parsed;
}

function requireValue(argv, index, arg) {
  const value = argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${arg}`);
  }
  return value;
}

function printUsage() {
  console.log(`Usage: node scripts/validate-product-docs.mjs [options]

Options:
  --docs-dir <path>    Product docs directory. Default: ${DEFAULT_DOCS_DIR}
  --app-routes <path>  React route source for missing-doc hints. Default: ${DEFAULT_APP_ROUTES_FILE}
  --route-registry <path>
                       Route registry source. Default: ${DEFAULT_ROUTE_REGISTRY_FILE}
  --check             Validate that ${DEFAULT_DOCS_DIR}/${INDEX_FILE} is up to date without writing
  --help              Show this help
`);
}

function listMarkdownFiles(dirPath) {
  if (!fs.existsSync(dirPath)) return [];

  const files = [];
  for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
    const entryPath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...listMarkdownFiles(entryPath));
    } else if (entry.isFile() && entry.name.toLowerCase().endsWith(".md")) {
      files.push(entryPath);
    }
  }
  return files;
}

function extractFrontmatter(text) {
  const match = text.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/);
  if (!match) return null;
  return { raw: match[1] };
}

function parseFrontmatter(raw) {
  const lines = raw.replace(/\r\n/g, "\n").split("\n");
  const data = {};
  const errors = [];

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (isIgnorableLine(line)) continue;

    const match = line.match(/^([A-Za-z][A-Za-z0-9_-]*):(?:\s*(.*))?$/);
    if (!match) {
      errors.push(`unsupported frontmatter line: ${line.trim()}`);
      continue;
    }

    const key = match[1];
    const value = match[2] ?? "";
    if (Object.prototype.hasOwnProperty.call(data, key)) {
      errors.push(`duplicate frontmatter key: ${key}`);
    }

    if (value.trim() !== "") {
      data[key] = parseScalar(value);
      continue;
    }

    const block = [];
    let nextIndex = index + 1;
    while (nextIndex < lines.length) {
      const nextLine = lines[nextIndex];
      if (/^[A-Za-z][A-Za-z0-9_-]*:\s*/.test(nextLine)) break;
      block.push(nextLine);
      nextIndex += 1;
    }

    data[key] = parseBlock(block);
    index = nextIndex - 1;
  }

  return { data, errors };
}

function parseBlock(lines) {
  const meaningful = lines.filter((line) => !isIgnorableLine(line));
  if (meaningful.length === 0) return "";

  if (meaningful.some((line) => line.trim().startsWith("- "))) {
    return parseListBlock(meaningful);
  }

  if (meaningful.every((line) => isKeyValue(line.trim()))) {
    const object = {};
    for (const line of meaningful) {
      const { key, value } = parseKeyValue(line.trim());
      object[key] = parseScalar(value);
    }
    return object;
  }

  return meaningful.map((line) => line.trim()).join("\n");
}

function parseListBlock(lines) {
  const items = [];

  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();
    if (!trimmed.startsWith("- ")) continue;

    const itemText = trimmed.slice(2).trim();
    if (!isKeyValue(itemText)) {
      items.push(parseScalar(itemText));
      continue;
    }

    const object = {};
    const first = parseKeyValue(itemText);
    object[first.key] = parseScalar(first.value);

    let nextIndex = index + 1;
    while (nextIndex < lines.length) {
      const nextTrimmed = lines[nextIndex].trim();
      if (nextTrimmed.startsWith("- ")) break;
      if (isKeyValue(nextTrimmed)) {
        const next = parseKeyValue(nextTrimmed);
        object[next.key] = parseScalar(next.value);
      }
      nextIndex += 1;
    }

    items.push(object);
    index = nextIndex - 1;
  }

  return items;
}

function parseScalar(value) {
  const trimmed = stripInlineComment(value.trim());
  if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
    return splitInlineList(trimmed.slice(1, -1)).map(parseScalar);
  }

  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }

  return trimmed;
}

function splitInlineList(value) {
  const parts = [];
  let current = "";
  let quote = "";

  for (const char of value) {
    if ((char === '"' || char === "'") && quote === "") {
      quote = char;
      current += char;
      continue;
    }

    if (char === quote) {
      quote = "";
      current += char;
      continue;
    }

    if (char === "," && quote === "") {
      parts.push(current.trim());
      current = "";
      continue;
    }

    current += char;
  }

  if (current.trim() !== "") parts.push(current.trim());
  return parts;
}

function stripInlineComment(value) {
  let quote = "";
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    if ((char === '"' || char === "'") && quote === "") {
      quote = char;
    } else if (char === quote) {
      quote = "";
    } else if (char === "#" && quote === "" && /\s/.test(value[index - 1] || "")) {
      return value.slice(0, index).trim();
    }
  }
  return value;
}

function buildProductDoc(filePath, frontmatter) {
  const explicitType = asString(frontmatter.type).toLowerCase();
  const rawDocType = asString(frontmatter.doc_type).toLowerCase();
  const rawType = explicitType || rawDocType;
  const canonicalType = TYPE_ALIASES.get(rawType) || "";
  const docTypeCanonical = TYPE_ALIASES.get(rawDocType) || "";
  const directRoute = firstPresent(frontmatter.route, frontmatter.path, frontmatter.url);
  const route = directRoute ? normalizeRoute(asString(directRoute)) : "";
  const routes = normalizeRoutes(firstPresent(frontmatter.routes, route));
  const elementId = firstStringFromFields(frontmatter, ELEMENT_FIELDS);
  const declaredElements = uniqueStrings(
    ELEMENT_COLLECTION_FIELDS.flatMap((field) => normalizeElementList(frontmatter[field])),
  );

  return {
    absPath: filePath,
    canonicalType,
    canonicalFor: normalizeStringList(frontmatter.canonical_for),
    coversRouteInventory: coversRouteInventory(frontmatter),
    declaredElements,
    docType: rawDocType,
    docTypeCanonical,
    elementId,
    frontmatter,
    relPath: toPosix(path.relative(root, filePath)),
    routes,
    sourceFiles: normalizeStringList(frontmatter.source_files),
    status: asString(frontmatter.status),
    title: asString(frontmatter.title),
    type: rawType,
  };
}

function validateProductDoc(doc) {
  const errors = [];

  if (!doc.type) errors.push(`${doc.relPath}: missing required field "type" or "doc_type"`);
  if (!doc.title) errors.push(`${doc.relPath}: missing required field "title"`);
  if (!doc.status) errors.push(`${doc.relPath}: missing required field "status"`);

  if (doc.type && !doc.canonicalType) {
    errors.push(`${doc.relPath}: unsupported type "${doc.type}"`);
    return errors;
  }

  if (doc.docType && !doc.docTypeCanonical) {
    errors.push(`${doc.relPath}: unsupported doc_type "${doc.docType}"`);
  }

  if (doc.canonicalType === "route" && doc.routes.length === 0) {
    errors.push(`${doc.relPath}: type "${doc.type}" requires "route"`);
  }

  if (doc.canonicalType === "element") {
    if (doc.routes.length === 0) {
      errors.push(`${doc.relPath}: type "${doc.type}" requires "route"`);
    }
    if (!doc.elementId) {
      errors.push(`${doc.relPath}: type "${doc.type}" requires one of ${ELEMENT_FIELDS.join(", ")}`);
    }
  }

  if (doc.canonicalType === "flow" && doc.routes.length === 0 && !frontmatterHasList(doc.frontmatter.steps)) {
    errors.push(`${doc.relPath}: type "${doc.type}" requires "routes", "route", or "steps"`);
  }

  if (doc.canonicalType === "api" && doc.routes.length === 0 && !asString(doc.frontmatter.endpoint)) {
    errors.push(`${doc.relPath}: type "${doc.type}" requires "route" or "endpoint"`);
  }

  if (doc.docType && ["compatibility", "current-state", "decision", "overview"].includes(doc.docTypeCanonical)) {
    if (!asString(doc.frontmatter.owner)) {
      errors.push(`${doc.relPath}: doc_type "${doc.docType}" requires "owner"`);
    }
    if (!asString(doc.frontmatter.last_updated)) {
      errors.push(`${doc.relPath}: doc_type "${doc.docType}" requires "last_updated"`);
    }
    if (!asString(doc.frontmatter.last_verified)) {
      errors.push(`${doc.relPath}: doc_type "${doc.docType}" requires "last_verified"`);
    }
    if (!frontmatterHasList(doc.frontmatter.canonical_for)) {
      errors.push(`${doc.relPath}: doc_type "${doc.docType}" requires "canonical_for"`);
    }
    if (!frontmatterHasList(doc.frontmatter.source_files)) {
      errors.push(`${doc.relPath}: doc_type "${doc.docType}" requires "source_files"`);
    }
  }

  return errors;
}

function coversRouteInventory(frontmatter) {
  const searchable = [
    asString(frontmatter.title),
    ...normalizeStringList(frontmatter.canonical_for),
    ...normalizeStringList(frontmatter.source_files),
  ].join(" ").toLowerCase();

  return searchable.includes("route") || searchable.includes("app/src/app.tsx");
}

function generateIndex({ docs, appRoutes, schemaErrors, docsDir }) {
  const appRouteSet = new Set(appRoutes);
  const routeRows = buildRouteRows(docs, appRoutes);
  const duplicateRows = buildDuplicateRows(docs);
  const missingHints = buildMissingHints(routeRows);
  const counts = countDocs(docs);
  const lines = [];

  lines.push("# Product Documentation Index");
  lines.push("");
  lines.push("This file is generated by `node scripts/validate-product-docs.mjs`.");
  lines.push("");
  lines.push("## Summary");
  lines.push("");
  lines.push(`- Product docs: ${docs.length}`);
  lines.push(`- Route docs: ${counts.route}`);
  lines.push(`- Element docs: ${counts.element}`);
  lines.push(`- Flow docs: ${counts.flow}`);
  lines.push(`- Current-state docs: ${counts.currentState}`);
  lines.push(`- Decision docs: ${counts.decision}`);
  lines.push(`- Compatibility docs: ${counts.compatibility}`);
  lines.push(`- Schema errors: ${schemaErrors.length}`);
  lines.push("");

  lines.push("## Route Map");
  lines.push("");
  lines.push("| Route | App route | Route docs | Route status | Declared elements | Element docs | Missing-doc hints |");
  lines.push("| --- | --- | ---: | --- | ---: | ---: | --- |");
  if (routeRows.length === 0) {
    lines.push("| _No routes found_ | no | 0 | - | 0 | 0 | Add route docs under `docs/product`. |");
  } else {
    for (const row of routeRows) {
      lines.push([
        tableCell(row.route),
        row.isAppRoute ? "yes" : "no",
        String(row.routeDocs.length),
        tableCell(formatStatuses(row.routeDocs)),
        String(row.declaredElements.length),
        String(row.elementDocs.length),
        tableCell(row.hints.length > 0 ? row.hints.join("; ") : "-"),
      ].join(" | ").replace(/^/, "| ").replace(/$/, " |"));
    }
  }
  lines.push("");

  lines.push("## Element Counts");
  lines.push("");
  lines.push("| Route | Declared in route docs | Element docs | Unique elements |");
  lines.push("| --- | ---: | ---: | ---: |");
  if (routeRows.length === 0) {
    lines.push("| _No routes found_ | 0 | 0 | 0 |");
  } else {
    for (const row of routeRows) {
      lines.push(`| ${tableCell(row.route)} | ${row.declaredElements.length} | ${row.elementDocs.length} | ${row.uniqueElements.length} |`);
    }
  }
  lines.push("");

  lines.push("## Duplicate Statuses");
  lines.push("");
  if (duplicateRows.length === 0) {
    lines.push("No duplicate route or element documentation statuses detected.");
  } else {
    lines.push("| Key | Docs | Statuses |");
    lines.push("| --- | --- | --- |");
    for (const row of duplicateRows) {
      lines.push(`| ${tableCell(row.key)} | ${tableCell(row.docs.join("<br>"))} | ${tableCell(row.statuses.join("<br>"))} |`);
    }
  }
  lines.push("");

  lines.push("## Missing-Doc Hints");
  lines.push("");
  if (missingHints.length === 0) {
    lines.push("No missing product-doc hints.");
  } else {
    for (const hint of missingHints) {
      lines.push(`- ${hint}`);
    }
  }
  lines.push("");

  lines.push("## Source Documents");
  lines.push("");
  lines.push("| File | Type | Doc type | Title | Status | Routes | Elements |");
  lines.push("| --- | --- | --- | --- | --- | --- | --- |");
  if (docs.length === 0) {
    lines.push("| _No source docs found_ | - | - | - | - | - | - |");
  } else {
    for (const doc of [...docs].sort((a, b) => comparePaths(a.relPath, b.relPath))) {
      const link = makeDocLink(doc, docsDir);
      lines.push([
        tableCell(link),
        tableCell(doc.type || "-"),
        tableCell(doc.docType || "-"),
        tableCell(doc.title || "-"),
        tableCell(doc.status || "-"),
        tableCell(doc.routes.length > 0 ? doc.routes.join(", ") : "-"),
        tableCell(formatDocElements(doc)),
      ].join(" | ").replace(/^/, "| ").replace(/$/, " |"));
    }
  }
  lines.push("");

  lines.push("## Schema Errors");
  lines.push("");
  if (schemaErrors.length === 0) {
    lines.push("None.");
  } else {
    for (const error of schemaErrors) {
      lines.push(`- ${error}`);
    }
  }
  lines.push("");

  if (appRouteSet.size === 0) {
    lines.push("> App route scan found no routes. Check `--app-routes` if missing-doc hints are unexpectedly empty.");
    lines.push("");
  }

  return `${lines.join("\n")}\n`;
}

function buildRouteRows(docs, appRoutes) {
  const routes = new Set(appRoutes);
  const routeInventoryDocs = docs.filter((doc) => doc.coversRouteInventory);
  for (const doc of docs) {
    for (const route of doc.routes) routes.add(route);
  }

  return [...routes].sort(compareRoutes).map((route) => {
    const explicitRouteDocs = docs.filter((doc) => doc.canonicalType === "route" && doc.routes.includes(route));
    const routeDocs = explicitRouteDocs.length > 0 ? explicitRouteDocs : routeInventoryDocs;
    const elementDocs = docs.filter((doc) => doc.canonicalType === "element" && doc.routes.includes(route));
    const declaredElements = uniqueStrings(routeDocs.flatMap((doc) => doc.declaredElements));
    const elementDocIds = uniqueStrings(elementDocs.map((doc) => doc.elementId).filter(Boolean));
    const uniqueElements = uniqueStrings([...declaredElements, ...elementDocIds]);
    const hints = [];
    const isAppRoute = appRoutes.includes(route);

    if (isAppRoute && routeDocs.length === 0) {
      hints.push("add route doc");
    }

    const missingElementDocs = declaredElements.filter((element) => !elementDocIds.includes(element));
    if (missingElementDocs.length > 0) {
      hints.push(`add element docs: ${missingElementDocs.join(", ")}`);
    }

    if (!isAppRoute && routeDocs.length > 0) {
      hints.push("route not found in app route scan");
    }

    return {
      declaredElements,
      elementDocs,
      hints,
      isAppRoute,
      route,
      routeDocs,
      uniqueElements,
    };
  });
}

function buildDuplicateRows(docs) {
  const groups = new Map();

  for (const doc of docs) {
    if (doc.canonicalType === "route") {
      for (const route of doc.routes) {
        addGroup(groups, `route:${route}`, doc);
      }
    }

    if (doc.canonicalType === "element" && doc.elementId) {
      for (const route of doc.routes) {
        addGroup(groups, `element:${route}:${doc.elementId}`, doc);
      }
    }
  }

  return [...groups.entries()]
    .filter(([, groupedDocs]) => groupedDocs.length > 1)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, groupedDocs]) => ({
      docs: groupedDocs.map((doc) => doc.relPath),
      key,
      statuses: groupedDocs.map((doc) => `${doc.status || "missing"} (${doc.relPath})`),
    }));
}

function buildMissingHints(routeRows) {
  return routeRows.flatMap((row) => row.hints.map((hint) => `\`${row.route}\`: ${hint}`));
}

function countDocs(docs) {
  return docs.reduce(
    (counts, doc) => {
      if (doc.coversRouteInventory && doc.canonicalType !== "route") {
        counts.route += 1;
      }

      if (doc.docTypeCanonical === "current-state") {
        counts.currentState += 1;
      } else if (doc.docTypeCanonical === "decision") {
        counts.decision += 1;
      } else if (doc.docTypeCanonical === "compatibility") {
        counts.compatibility += 1;
      }

      if (doc.canonicalType && Object.prototype.hasOwnProperty.call(counts, doc.canonicalType)) {
        counts[doc.canonicalType] += 1;
      }
      return counts;
    },
    { api: 0, compatibility: 0, currentState: 0, decision: 0, element: 0, feature: 0, flow: 0, overview: 0, route: 0 },
  );
}

function readRouteRegistry(filePath) {
  const relPath = toPosix(path.relative(root, filePath));
  const errors = [];
  const routes = [];

  if (!fs.existsSync(filePath)) {
    errors.push(`${relPath}: route registry file not found`);
    return { errors, filePath, routes };
  }

  const text = fs.readFileSync(filePath, "utf8");
  const registryMatch = text.match(/export const PRODUCT_ROUTES\s*=\s*\[([\s\S]*?)\]\s+as const/);
  if (!registryMatch) {
    errors.push(`${relPath}: PRODUCT_ROUTES export not found`);
  }
  const registryText = registryMatch ? registryMatch[1] : "";
  const routeBlocks = registryText.match(/\{\s*[\s\S]*?\n\s*\}/g) ?? [];

  for (const block of routeBlocks) {
    const id = extractStringProperty(block, "id");
    const rawPath = extractStringProperty(block, "path");
    const viewId = extractStringProperty(block, "viewId");
    const navButtonDocId = extractStringProperty(block, "navButtonDocId");
    const shortcut = extractStringProperty(block, "shortcut");
    const shortcutKey = extractStringProperty(block, "shortcutKey");

    if (!rawPath && !viewId) continue;

    const routePath = normalizeRoute(rawPath);
    const label = id || routePath || "(missing route id)";
    if (!id) errors.push(`${relPath}: ${label}: missing route id`);
    if (!routePath) errors.push(`${relPath}: ${label}: missing route path`);
    if (!viewId || !viewId.startsWith("view:")) {
      errors.push(`${relPath}: ${label}: route registry entry requires view:* id`);
    }
    if (routePath && viewId && viewId !== `view:${routePath}`) {
      errors.push(`${relPath}: ${label}: viewId "${viewId}" does not match route path "${routePath}"`);
    }
    if (!navButtonDocId || !navButtonDocId.startsWith("button:/app/top-navigation/")) {
      errors.push(`${relPath}: ${label}: route registry entry requires top-navigation button id`);
    }
    if (!shortcut || !shortcutKey) {
      errors.push(`${relPath}: ${label}: route registry entry requires shortcut and shortcutKey`);
    }

    if (id && routePath && viewId) {
      routes.push({ id, navButtonDocId, path: routePath, shortcut, shortcutKey, viewId });
    }
  }

  if (routes.length === 0) {
    errors.push(`${relPath}: no route registry entries found`);
  }

  errors.push(...duplicateRouteRegistryValues(relPath, routes, "id"));
  errors.push(...duplicateRouteRegistryValues(relPath, routes, "path"));
  errors.push(...duplicateRouteRegistryValues(relPath, routes, "viewId"));
  errors.push(...duplicateRouteRegistryValues(relPath, routes, "shortcutKey"));

  return { errors, filePath, routes };
}

function extractStringProperty(block, name) {
  const pattern = new RegExp(`\\b${name}:\\s*(['"\`])([^'"\`]+)\\1`);
  const match = block.match(pattern);
  return match ? match[2].trim() : "";
}

function duplicateRouteRegistryValues(relPath, routes, field) {
  const errors = [];
  const seen = new Map();

  for (const route of routes) {
    const value = asString(route[field]);
    if (!value) continue;
    if (seen.has(value)) {
      errors.push(`${relPath}: duplicate route registry ${field} "${value}" in ${seen.get(value)} and ${route.id}`);
    } else {
      seen.set(value, route.id);
    }
  }

  return errors;
}

function validateRouteRegistryCatalog(routeRegistry, catalog) {
  const errors = [];
  const views = catalog.byKind.get("view") ?? [];
  const buttons = catalog.byKind.get("button") ?? [];
  const viewsById = new Map(views.map((record) => [asString(record.id), record]));
  const buttonIds = new Set(buttons.map((record) => asString(record.id)).filter(Boolean));
  const relPath = toPosix(path.relative(root, routeRegistry.filePath));

  for (const route of routeRegistry.routes) {
    const view = viewsById.get(route.viewId);
    if (!view) {
      errors.push(`${relPath}: ${route.id}: registry viewId missing from catalog: ${route.viewId}`);
    } else if (normalizeRoute(view.route) !== route.path) {
      errors.push(`${view.__relPath}: ${route.viewId}: catalog route "${asString(view.route)}" does not match registry path "${route.path}"`);
    }

    if (!buttonIds.has(route.navButtonDocId)) {
      errors.push(`${relPath}: ${route.id}: registry top-navigation button missing from catalog: ${route.navButtonDocId}`);
    }
  }

  return errors;
}

function mergeRoutes(...routeLists) {
  return uniqueStrings(routeLists.flat())
    .map(normalizeRoute)
    .filter(Boolean)
    .sort(compareRoutes);
}

function readProductCatalog({ catalogDir, schemaDir, appRoutes }) {
  const errors = [];
  const records = [];
  const schemas = loadCatalogSchemas(schemaDir, errors);

  if (!fs.existsSync(catalogDir)) {
    return { byId: new Map(), byKind: new Map(), errors, records };
  }

  const yamlFiles = listYamlFiles(catalogDir).sort(comparePaths);
  for (const filePath of yamlFiles) {
    const relPath = toPosix(path.relative(root, filePath));
    const relCatalogPath = toPosix(path.relative(catalogDir, filePath));
    const collectionName = relCatalogPath.split("/")[0];
    const config = CATALOG_KIND_CONFIG[collectionName];

    if (!config) {
      errors.push(`${relPath}: unsupported catalog collection "${collectionName}"`);
      continue;
    }

    let parsed;
    try {
      parsed = parseYamlFile(filePath);
    } catch (error) {
      errors.push(`${relPath}: ${error.message}`);
      continue;
    }

    const extracted = extractCatalogRecords(parsed, config);
    if (extracted.length === 0) {
      errors.push(`${relPath}: no catalog records found`);
      continue;
    }

    for (const item of extracted) {
      if (!item || typeof item !== "object" || Array.isArray(item)) {
        errors.push(`${relPath}: catalog item must be an object`);
        continue;
      }

      const record = {
        ...item,
        __absPath: filePath,
        __kind: config.kind,
        __relPath: relPath,
      };
      records.push(record);

      const schema = schemas.get(config.schema);
      if (schema) {
        errors.push(...validateCatalogRecordAgainstSchema(record, schema));
      }
      errors.push(...validateCatalogRecordPaths(record));
      errors.push(...validateCatalogRoute(record, appRoutes));
    }
  }

  const byId = new Map();
  const byKind = new Map();
  for (const record of records) {
    if (!byKind.has(record.__kind)) byKind.set(record.__kind, []);
    byKind.get(record.__kind).push(record);

    const id = asString(record.id);
    if (!id) continue;
    if (byId.has(id)) {
      errors.push(`${record.__relPath}: duplicate catalog id "${id}" also declared in ${byId.get(id).__relPath}`);
    } else {
      byId.set(id, record);
    }
  }

  errors.push(...validateCatalogReferences(records, byId));
  errors.push(...validateCatalogSelectors(records));

  return { byId, byKind, errors, records };
}

function listYamlFiles(dirPath) {
  if (!fs.existsSync(dirPath)) return [];

  const files = [];
  for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
    const entryPath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      files.push(...listYamlFiles(entryPath));
    } else if (entry.isFile() && /\.(ya?ml)$/i.test(entry.name)) {
      files.push(entryPath);
    }
  }
  return files;
}

function loadCatalogSchemas(schemaDir, errors) {
  const schemas = new Map();
  if (!fs.existsSync(schemaDir)) return schemas;

  for (const config of Object.values(CATALOG_KIND_CONFIG)) {
    const schemaPath = path.join(schemaDir, config.schema);
    if (!fs.existsSync(schemaPath)) {
      errors.push(`${toPosix(path.relative(root, schemaPath))}: missing catalog schema`);
      continue;
    }

    try {
      schemas.set(config.schema, JSON.parse(fs.readFileSync(schemaPath, "utf8")));
    } catch (error) {
      errors.push(`${toPosix(path.relative(root, schemaPath))}: invalid JSON schema: ${error.message}`);
    }
  }

  return schemas;
}

function parseYamlFile(filePath) {
  const text = fs.readFileSync(filePath, "utf8");
  const parser = createYamlParser(text, toPosix(path.relative(root, filePath)));
  return parser.parse();
}

function createYamlParser(text, relPath) {
  const lines = text.replace(/\r\n/g, "\n").split("\n")
    .map((raw, index) => ({ raw, number: index + 1 }))
    .filter(({ raw }) => {
      const trimmed = raw.trim();
      return trimmed !== "" && !trimmed.startsWith("#");
    })
    .map(({ raw, number }) => {
      if (raw.includes("\t")) {
        throw new Error(`line ${number}: tabs are not supported in catalog YAML`);
      }
      const indent = raw.match(/^ */)[0].length;
      return { indent, number, text: raw.slice(indent).replace(/\s+$/, "") };
    });

  let cursor = 0;

  return {
    parse() {
      if (lines.length === 0) return {};
      const document = parseNode(lines[0].indent);
      if (cursor < lines.length) {
        throw new Error(`line ${lines[cursor].number}: unexpected content`);
      }
      return document;
    },
  };

  function parseNode(indent) {
    if (cursor >= lines.length) return null;
    const line = lines[cursor];
    if (line.indent < indent) return null;
    if (line.indent > indent) {
      throw new Error(`line ${line.number}: unexpected indent in ${relPath}`);
    }
    return line.text.startsWith("- ") ? parseSequence(indent) : parseMapping(indent);
  }

  function parseMapping(indent) {
    const object = {};

    while (cursor < lines.length) {
      const line = lines[cursor];
      if (line.indent < indent) break;
      if (line.indent > indent) {
        throw new Error(`line ${line.number}: unexpected nested mapping content`);
      }
      if (line.text.startsWith("- ")) break;

      const { key, value } = parseYamlKeyValue(line);
      if (Object.prototype.hasOwnProperty.call(object, key)) {
        throw new Error(`line ${line.number}: duplicate key "${key}"`);
      }

      cursor += 1;
      if (value === "") {
        object[key] = cursor < lines.length && lines[cursor].indent > indent
          ? parseNode(lines[cursor].indent)
          : "";
      } else {
        object[key] = parseYamlScalar(value);
      }
    }

    return object;
  }

  function parseSequence(indent) {
    const items = [];

    while (cursor < lines.length) {
      const line = lines[cursor];
      if (line.indent < indent) break;
      if (line.indent > indent) {
        throw new Error(`line ${line.number}: unexpected nested sequence content`);
      }
      if (!line.text.startsWith("- ")) break;

      const itemText = line.text.slice(2).trim();
      cursor += 1;

      if (itemText === "") {
        items.push(cursor < lines.length && lines[cursor].indent > indent ? parseNode(lines[cursor].indent) : null);
        continue;
      }

      if (isYamlInlineObjectStart(itemText)) {
        const object = {};
        const first = parseYamlKeyValue({ text: itemText, number: line.number });
        object[first.key] = first.value === ""
          ? (cursor < lines.length && lines[cursor].indent > indent ? parseNode(lines[cursor].indent) : "")
          : parseYamlScalar(first.value);

        if (cursor < lines.length && lines[cursor].indent > indent) {
          const nested = parseNode(lines[cursor].indent);
          if (!nested || typeof nested !== "object" || Array.isArray(nested)) {
            throw new Error(`line ${line.number}: object list item has non-object nested content`);
          }
          Object.assign(object, nested);
        }
        items.push(object);
      } else {
        items.push(parseYamlScalar(itemText));
      }
    }

    return items;
  }
}

function parseYamlKeyValue(line) {
  const match = line.text.match(/^([A-Za-z0-9_.-]+):(?:\s*(.*))?$/);
  if (!match) {
    throw new Error(`line ${line.number}: unsupported YAML line "${line.text}"`);
  }
  return { key: match[1], value: match[2] ?? "" };
}

function isYamlInlineObjectStart(value) {
  return /^[A-Za-z0-9_.-]+:(?:\s+|$)/.test(value);
}

function parseYamlScalar(value) {
  const trimmed = stripInlineComment(value.trim());

  if (trimmed === "[]") return [];
  if (trimmed === "{}") return {};
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (trimmed === "null") return null;

  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }

  return trimmed;
}

function extractCatalogRecords(parsed, config) {
  if (Array.isArray(parsed)) return parsed;
  if (!parsed || typeof parsed !== "object") return [];
  if (Array.isArray(parsed[config.wrapper])) return parsed[config.wrapper];
  if (asString(parsed.id)) return [parsed];
  return [];
}

function validateCatalogRecordAgainstSchema(record, schema) {
  const errors = [];
  const relPath = record.__relPath;
  const idForMessage = asString(record.id) || "(missing id)";

  for (const field of schema.required ?? []) {
    if (!Object.prototype.hasOwnProperty.call(record, field)) {
      errors.push(`${relPath}: ${idForMessage}: missing required catalog field "${field}"`);
    }
  }

  for (const [field, definition] of Object.entries(schema.properties ?? {})) {
    if (!Object.prototype.hasOwnProperty.call(record, field)) continue;
    const value = record[field];
    errors.push(...validateSchemaValue(value, definition, `${relPath}: ${idForMessage}: ${field}`));
  }

  return errors;
}

function validateSchemaValue(value, definition, label) {
  const errors = [];
  if (!definition || !definition.type) return errors;

  if (!matchesJsonSchemaType(value, definition.type)) {
    errors.push(`${label} must be ${definition.type}`);
    return errors;
  }

  if (definition.pattern && typeof value === "string" && !(new RegExp(definition.pattern).test(value))) {
    errors.push(`${label} does not match pattern ${definition.pattern}`);
  }

  if (definition.type === "array" && definition.items) {
    value.forEach((item, index) => {
      errors.push(...validateSchemaValue(item, definition.items, `${label}[${index}]`));
    });
  }

  return errors;
}

function matchesJsonSchemaType(value, expectedType) {
  if (expectedType === "array") return Array.isArray(value);
  if (expectedType === "boolean") return typeof value === "boolean";
  if (expectedType === "object") return Boolean(value) && typeof value === "object" && !Array.isArray(value);
  if (expectedType === "string") return typeof value === "string" && value.trim() !== "";
  return true;
}

function validateCatalogRecordPaths(record) {
  const errors = [];
  for (const field of ["source_files", "tests"]) {
    const values = Array.isArray(record[field]) ? record[field] : [];
    for (const value of values) {
      const relValue = asString(value);
      if (!relValue) continue;
      const absPath = path.resolve(root, relValue);
      if (!fs.existsSync(absPath)) {
        errors.push(`${record.__relPath}: ${asString(record.id)}: ${field} path does not exist: ${relValue}`);
      }
    }
  }
  return errors;
}

function validateCatalogRoute(record, appRoutes) {
  const route = asString(record.route);
  if (!route || !route.startsWith("/")) return [];
  if (!["view", "element", "button", "flow"].includes(record.__kind)) return [];
  if (!appRoutes.includes(normalizeRoute(route))) {
    return [`${record.__relPath}: ${asString(record.id)}: route "${route}" is not found in ${DEFAULT_APP_ROUTES_FILE}`];
  }
  return [];
}

function validateCatalogReferences(records, byId) {
  const errors = [];
  const byKind = new Map();
  for (const record of records) {
    if (!byKind.has(record.__kind)) byKind.set(record.__kind, []);
    byKind.get(record.__kind).push(record);
  }

  const elementIds = new Set((byKind.get("element") ?? []).map((record) => asString(record.id)).filter(Boolean));
  const buttonIds = new Set((byKind.get("button") ?? []).map((record) => asString(record.id)).filter(Boolean));
  const infoIds = new Set((byKind.get("info") ?? []).map((record) => asString(record.id)).filter(Boolean));
  const viewIds = new Set((byKind.get("view") ?? []).map((record) => asString(record.id)).filter(Boolean));
  const domainIds = new Set((byKind.get("domain") ?? []).map((record) => asString(record.id)).filter(Boolean));

  for (const record of records) {
    const id = asString(record.id);
    if (!id) continue;

    if (!id.startsWith(`${record.__kind}:`) && !(record.__kind === "design-system" && id.startsWith("design-system:"))) {
      errors.push(`${record.__relPath}: ${id}: id prefix does not match catalog kind ${record.__kind}`);
    }

    if (record.__kind === "view") {
      errors.push(...missingReferences(record, "owned_elements", elementIds));
      errors.push(...missingReferences(record, "primary_actions", buttonIds));
    }

    if (record.__kind === "element") {
      if (!viewIds.has(asString(record.parent_view))) {
        errors.push(`${record.__relPath}: ${id}: parent_view not found: ${asString(record.parent_view)}`);
      }
      errors.push(...missingReferences(record, "buttons", buttonIds));
      errors.push(...missingReferences(record, "infos", infoIds));
    }

    if (record.__kind === "button" && !elementIds.has(asString(record.owner_element))) {
      errors.push(`${record.__relPath}: ${id}: owner_element not found: ${asString(record.owner_element)}`);
    }

    if (record.__kind === "info" && !elementIds.has(asString(record.owner_element))) {
      errors.push(`${record.__relPath}: ${id}: owner_element not found: ${asString(record.owner_element)}`);
    }

    if (record.__kind === "flow") {
      errors.push(...missingReferences(record, "domain_objects", domainIds));
    }

    if (!byId.has(id)) {
      errors.push(`${record.__relPath}: ${id}: internal id map error`);
    }
  }

  return errors;
}

function missingReferences(record, field, knownIds) {
  const errors = [];
  const values = Array.isArray(record[field]) ? record[field] : [];
  for (const value of values) {
    const ref = asString(value);
    if (ref && !knownIds.has(ref)) {
      errors.push(`${record.__relPath}: ${asString(record.id)}: ${field} reference not found: ${ref}`);
    }
  }
  return errors;
}

function validateCatalogSelectors(records) {
  const errors = [];
  for (const record of records) {
    if (record.__kind !== "button") continue;
    const selector = asString(record.selector);
    const docIdMatch = selector.match(/^\[data-doc-id=["']([^"']+)["']\]$/);
    if (!docIdMatch) continue;

    const docId = docIdMatch[1];
    const sourceFiles = Array.isArray(record.source_files) ? record.source_files : [];
    const sourceTexts = sourceFiles
      .map((sourceFile) => path.resolve(root, asString(sourceFile)))
      .filter((sourceFile) => fs.existsSync(sourceFile) && /\.(tsx?|jsx?)$/i.test(sourceFile))
      .map((sourceFile) => fs.readFileSync(sourceFile, "utf8"));

    if (sourceTexts.length === 0) continue;
    const needleDouble = `data-doc-id="${docId}"`;
    const needleSingle = `data-doc-id='${docId}'`;
    const hasLiteralSelector = sourceTexts.some((text) => text.includes(needleDouble) || text.includes(needleSingle));
    const hasDynamicSelector = sourceTexts.some((text) => text.includes("data-doc-id={"))
      && sourceTexts.some((text) => text.includes(`'${docId}'`) || text.includes(`"${docId}"`) || text.includes(`\`${docId}\``));

    if (!hasLiteralSelector && !hasDynamicSelector) {
      errors.push(`${record.__relPath}: ${asString(record.id)}: selector data-doc-id is not present in source_files`);
    }
  }
  return errors;
}

function generateCatalogMarkdown(catalog) {
  const counts = {};
  for (const record of catalog.records) {
    counts[record.__kind] = (counts[record.__kind] ?? 0) + 1;
  }

  const lines = [];
  lines.push("# Product Catalog");
  lines.push("");
  lines.push("This file is generated by `node scripts/validate-product-docs.mjs` from `docs/product/catalog/**/*.yaml`.");
  lines.push("");
  lines.push("## Summary");
  lines.push("");
  lines.push(`- Catalog records: ${catalog.records.length}`);
  for (const kind of ["view", "element", "button", "info", "flow", "domain", "design-system"]) {
    lines.push(`- ${kind}: ${counts[kind] ?? 0}`);
  }
  lines.push(`- Catalog errors: ${catalog.errors.length}`);
  lines.push("");

  for (const kind of ["view", "element", "button", "info", "flow", "domain", "design-system"]) {
    const records = (catalog.byKind.get(kind) ?? []).sort((left, right) => asString(left.id).localeCompare(asString(right.id)));
    lines.push(`## ${catalogKindTitle(kind)}`);
    lines.push("");
    if (records.length === 0) {
      lines.push("_No records._");
      lines.push("");
      continue;
    }

    lines.push("| ID | Title / Component | Route | Source |");
    lines.push("| --- | --- | --- | --- |");
    for (const record of records) {
      const title = asString(record.title) || asString(record.component) || asString(record.type) || "-";
      const route = asString(record.route) || "-";
      const source = makeCatalogSourceLink(record);
      lines.push(`| ${tableCell(asString(record.id))} | ${tableCell(title)} | ${tableCell(route)} | ${tableCell(source)} |`);
    }
    lines.push("");
  }

  lines.push("## Catalog Errors");
  lines.push("");
  if (catalog.errors.length === 0) {
    lines.push("None.");
  } else {
    for (const error of catalog.errors) {
      lines.push(`- ${error}`);
    }
  }
  lines.push("");

  return `${lines.join("\n")}\n`;
}

function catalogKindTitle(kind) {
  return kind
    .split("-")
    .map((part) => part.slice(0, 1).toUpperCase() + part.slice(1))
    .join(" ");
}

function makeCatalogSourceLink(record) {
  const rel = toPosix(path.relative(path.join(docsDir, GENERATED_DIR), record.__absPath));
  return `[${record.__relPath}](${encodeURI(rel).replace(/#/g, "%23")})`;
}

function readAppRoutes(filePath) {
  if (!fs.existsSync(filePath)) return [];

  const text = fs.readFileSync(filePath, "utf8");
  const routes = new Set();

  if (/<Route\b[^>]*\bindex\b/.test(text)) {
    routes.add("/");
  }

  const routePattern = /<Route\b[^>]*\bpath=(?:"([^"]+)"|'([^']+)')/g;
  let match;
  while ((match = routePattern.exec(text)) !== null) {
    const rawRoute = match[1] || match[2] || "";
    if (!rawRoute || rawRoute === "*" || rawRoute.includes("*")) continue;
    routes.add(normalizeRoute(rawRoute));
  }

  return [...routes].sort(compareRoutes);
}

function addGroup(groups, key, doc) {
  if (!groups.has(key)) groups.set(key, []);
  groups.get(key).push(doc);
}

function formatStatuses(docs) {
  const statuses = uniqueStrings(docs.map((doc) => doc.status).filter(Boolean));
  return statuses.length > 0 ? statuses.join(", ") : "-";
}

function formatDocElements(doc) {
  if (doc.canonicalType === "element" && doc.elementId) return doc.elementId;
  if (doc.declaredElements.length > 0) return doc.declaredElements.join(", ");
  return "-";
}

function makeDocLink(doc, docsDir) {
  const rel = toPosix(path.relative(docsDir, doc.absPath));
  return `[${doc.relPath}](${encodeURI(rel).replace(/#/g, "%23")})`;
}

function normalizeRoutes(value) {
  return uniqueStrings(normalizeStringList(value).map(normalizeRoute).filter(Boolean));
}

function normalizeRoute(route) {
  const value = asString(route);
  if (!value) return "";
  if (value === "/") return "/";
  if (value.startsWith("http://") || value.startsWith("https://")) return value;
  return `/${value.replace(/^\/+/, "")}`.replace(/\/{2,}/g, "/");
}

function normalizeElementList(value) {
  if (Array.isArray(value)) {
    return value.flatMap((item) => {
      if (typeof item === "string") return splitLooseList(item);
      if (item && typeof item === "object") {
        return [firstStringFromFields(item, ELEMENT_FIELDS.concat(["title"]))].filter(Boolean);
      }
      return [];
    });
  }

  if (value && typeof value === "object") {
    return Object.entries(value).map(([key, itemValue]) => {
      if (itemValue && typeof itemValue === "object") {
        return firstStringFromFields(itemValue, ELEMENT_FIELDS.concat(["title"])) || key;
      }
      return key;
    });
  }

  return normalizeStringList(value);
}

function normalizeStringList(value) {
  if (Array.isArray(value)) {
    return value.flatMap(normalizeStringList);
  }
  return splitLooseList(asString(value));
}

function splitLooseList(value) {
  if (!value) return [];
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function firstStringFromFields(object, fields) {
  for (const field of fields) {
    const value = asString(object[field]);
    if (value) return value;
  }
  return "";
}

function firstPresent(...values) {
  for (const value of values) {
    if (value !== undefined && value !== null && value !== "") return value;
  }
  return "";
}

function frontmatterHasList(value) {
  return Array.isArray(value) ? value.length > 0 : Boolean(asString(value));
}

function asString(value) {
  if (value === undefined || value === null) return "";
  if (Array.isArray(value)) return value.map(asString).filter(Boolean).join(", ");
  if (typeof value === "object") return "";
  return String(value).trim();
}

function uniqueStrings(values) {
  const seen = new Set();
  const unique = [];
  for (const value of values) {
    const stringValue = asString(value);
    if (!stringValue || seen.has(stringValue)) continue;
    seen.add(stringValue);
    unique.push(stringValue);
  }
  return unique;
}

function isIgnorableLine(line) {
  const trimmed = line.trim();
  return trimmed === "" || trimmed.startsWith("#");
}

function isKeyValue(value) {
  return /^[A-Za-z][A-Za-z0-9_-]*:\s*/.test(value);
}

function parseKeyValue(value) {
  const match = value.match(/^([A-Za-z][A-Za-z0-9_-]*):\s*(.*)$/);
  return { key: match[1], value: match[2] };
}

function tableCell(value) {
  return String(value)
    .replace(/\r?\n/g, "<br>")
    .replace(/\|/g, "\\|");
}

function toPosix(filePath) {
  return filePath.split(path.sep).join("/");
}

function comparePaths(left, right) {
  return toPosix(left).localeCompare(toPosix(right));
}

function compareRoutes(left, right) {
  if (left === right) return 0;
  if (left === "/") return -1;
  if (right === "/") return 1;
  return left.localeCompare(right);
}
