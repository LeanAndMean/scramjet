#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");
const docsPath = path.join(repoRoot, "docs", "pi-api-surface.md");
const packageJson = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));

const packages = ["pi-agent-core", "pi-ai", "pi-coding-agent", "pi-tui"];

const generatedFileNames = new Set(["models.generated.d.ts"]);

function createProgram(entry) {
	return ts.createProgram([entry], {
		allowJs: false,
		declaration: true,
		esModuleInterop: true,
		module: ts.ModuleKind.NodeNext,
		moduleResolution: ts.ModuleResolutionKind.NodeNext,
		skipLibCheck: true,
		target: ts.ScriptTarget.ES2022,
	});
}

function declarationKind(declaration) {
	if (ts.isExportSpecifier(declaration)) return "re-export";
	if (ts.isInterfaceDeclaration(declaration)) return "interface";
	if (ts.isTypeAliasDeclaration(declaration)) return "type";
	if (ts.isFunctionDeclaration(declaration)) return "function";
	if (ts.isClassDeclaration(declaration)) return "class";
	if (ts.isEnumDeclaration(declaration)) return "enum";
	if (ts.isVariableDeclaration(declaration) || ts.isVariableStatement(declaration)) return "const";
	if (ts.isModuleDeclaration(declaration)) return "namespace";
	return ts.SyntaxKind[declaration.kind] ?? "export";
}

function declarationText(declaration) {
	let node = declaration;
	if (ts.isVariableDeclaration(declaration)) node = declaration.parent.parent;
	if (ts.isExportSpecifier(declaration)) node = declaration.parent.parent;

	const sourceFile = declaration.getSourceFile();
	const comments = (ts.getLeadingCommentRanges(sourceFile.text, node.pos) ?? [])
		.map((range) => sourceFile.text.slice(range.pos, range.end).trim())
		.join("\n");
	const text = node.getText(sourceFile).trim();
	return comments ? `${comments}\n${text}` : text;
}

function isInsidePackage(packageRoot, declaration) {
	const sourcePath = path.resolve(declaration.getSourceFile().fileName);
	return sourcePath.startsWith(path.resolve(packageRoot) + path.sep);
}

function moduleName(packageRoot, declaration) {
	const sourceFile = declaration.getSourceFile();
	const packageDist = path.join(packageRoot, "dist");
	const sourcePath = path.resolve(sourceFile.fileName);

	if (sourcePath.startsWith(packageDist + path.sep)) {
		return path
			.relative(packageDist, sourcePath)
			.replace(/\\/g, "/")
			.replace(/\.d\.ts$/, "");
	}

	const nodeModulesIndex = sourcePath.lastIndexOf(`${path.sep}node_modules${path.sep}`);
	if (nodeModulesIndex !== -1) {
		return `external/${sourcePath
			.slice(nodeModulesIndex + "/node_modules/".length)
			.replace(/\\/g, "/")
			.replace(/\.d\.ts$/, "")}`;
	}

	return `external/${path
		.relative(repoRoot, sourcePath)
		.replace(/\\/g, "/")
		.replace(/\.d\.ts$/, "")}`;
}

function resolveExportSymbol(checker, symbol) {
	if ((symbol.flags & ts.SymbolFlags.Alias) !== 0) {
		return checker.getAliasedSymbol(symbol);
	}
	return symbol;
}

function collectPackageExports(packageName) {
	const packageRoot = path.join(repoRoot, "node_modules", "@earendil-works", packageName);
	const entry = path.join(packageRoot, "dist", "index.d.ts");

	if (!fs.existsSync(entry)) {
		throw new Error(`Missing declaration entry for ${packageName}: ${entry}`);
	}

	const program = createProgram(entry);
	const checker = program.getTypeChecker();
	const sourceFile = program.getSourceFile(entry);
	const moduleSymbol = checker.getSymbolAtLocation(sourceFile);

	if (!moduleSymbol) {
		throw new Error(`Unable to resolve module symbol for ${entry}`);
	}

	const entries = [];
	for (const exportSymbol of checker.getExportsOfModule(moduleSymbol)) {
		const symbol = resolveExportSymbol(checker, exportSymbol);
		const declarations = symbol.declarations ?? [];
		const filteredDeclarations = declarations.filter(
			(declaration) => !generatedFileNames.has(path.basename(declaration.getSourceFile().fileName)),
		);

		if (filteredDeclarations.length === 0) continue;

		const publicDeclarations = filteredDeclarations.some((declaration) => isInsidePackage(packageRoot, declaration))
			? filteredDeclarations
			: (exportSymbol.declarations ?? filteredDeclarations);
		const primaryDeclaration = filteredDeclarations[0];
		const displayDeclaration = publicDeclarations[0];
		entries.push({
			name: exportSymbol.getName(),
			kind: declarationKind(displayDeclaration),
			module: moduleName(packageRoot, primaryDeclaration),
			text: [...new Set(publicDeclarations.map(declarationText))].join("\n\n"),
		});
	}

	return entries.sort((a, b) => a.module.localeCompare(b.module) || a.name.localeCompare(b.name));
}

function slug(value) {
	return value
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-|-$/g, "");
}

function renderPackage(packageName, entries) {
	const lines = [`## @earendil-works/${packageName}`, ""];
	if (entries.length === 0) {
		lines.push("No public exports found.", "");
		return lines.join("\n");
	}

	const modules = new Map();
	for (const entry of entries) {
		const moduleEntries = modules.get(entry.module) ?? [];
		moduleEntries.push(entry);
		modules.set(entry.module, moduleEntries);
	}

	for (const [module, moduleEntries] of [...modules.entries()].sort(([a], [b]) => a.localeCompare(b))) {
		lines.push(`### ${module}`, "");
		for (const entry of moduleEntries) {
			lines.push(`#### ${entry.name}`, "", `Kind: ${entry.kind}`, "", "```ts", entry.text, "```", "");
		}
	}

	return lines.join("\n");
}

function renderDocument(packageEntries) {
	const piBaseVersion = packageJson.pi?.piBaseVersion ?? "unknown";
	const piTestedVersion = packageJson.pi?.piTestedVersion ?? "unknown";
	const lines = [
		"# Pi API Surface",
		"",
		`Generated from Pi ${piBaseVersion} / pi-coding-agent ${piTestedVersion}.`,
		"",
		"This file is generated from installed TypeScript declaration files. Do not edit it directly; run `node scripts/generate-pi-api-surface.js` instead.",
		"",
		"## Table of contents",
		"",
	];

	for (const packageName of packages) {
		lines.push(`- [@earendil-works/${packageName}](#${slug(`@earendil-works/${packageName}`)})`);
	}
	lines.push("");

	for (const [packageName, entries] of packageEntries) {
		lines.push(renderPackage(packageName, entries));
	}

	return `${lines.join("\n").trim()}\n`;
}

const packageEntries = packages.map((packageName) => [packageName, collectPackageExports(packageName)]);
fs.writeFileSync(docsPath, renderDocument(packageEntries));
console.log(`Wrote ${path.relative(repoRoot, docsPath)}`);
