#!/usr/bin/env bun
import { Glob } from "bun";
import { readFileSync } from "node:fs";
import { join, dirname, resolve, relative } from "node:path";

const CONFIG = {
	DOCS_DIR: "./docs/pages",
	PUBLIC_DIR: "./docs/public",
	REPORT_PATH: "links-report.json",
	FILE_PATTERNS: "**/*.{md,mdx}",
	MARKDOWN_EXTENSIONS: /\.(md|mdx)$/,
} as const;

interface BrokenLink {
	file: string;
	link: string;
	line: number;
	reason: string;
}

interface LinkCheckReport {
	timestamp: string;
	totalFiles: number;
	totalLinks: number;
	brokenLinks: Array<BrokenLink>;
	summary: {
		brokenCount: number;
		validCount: number;
	};
}

main();

async function main() {
	try {
		const report = await checkLinks();
		await saveReport(report);
		displayResults(report);

		process.exit(report.summary.brokenCount > 0 ? 1 : 0);
	} catch (error) {
		console.error("\n❌ Fatal error during link checking:");

		if (error instanceof Error) {
			console.error(`   ${error.message}`);
			if (error.stack) {
				[console.error("\nStack trace:"), console.error(error.stack)];
			}
		} else console.error(error);

		process.exit(2);
	}
}

async function checkLinks(): Promise<LinkCheckReport> {
	console.log("🔍 Finding markdown files...");
	const files = await getAllMarkdownFiles();
	console.log(`📄 Found ${files.length} markdown files`);

	console.log("🔍 Finding public assets...");
	const publicAssets = await getAllPublicAssets();
	console.log(`🖼️  Found ${publicAssets.length} public assets`);

	console.log("🗺️  Building file path map...");
	const pathMap = buildFilePathMap(files, publicAssets);
	console.log(`📍 Mapped ${pathMap.size} possible paths`);

	const brokenLinks: BrokenLink[] = [];
	let totalLinks = 0;

	console.log("🔗 Checking links in files...");

	for (let index = 0; index < files.length; index++) {
		const file = files[index];

		try {
			const content = readFileSync(file, "utf-8");
			const links = extractLinksFromMarkdown(content);

			for (const { link, line } of links) {
				totalLinks++;
				const error = validateLink(link, file, pathMap);

				if (error) {
					brokenLinks.push({
						file: relative(process.cwd(), file),
						link,
						line,
						reason: error,
					});
				}
			}
		} catch (error) {
			console.error(`\nError reading ${file}:`, error);
		}
	}

	console.log("\n✅ Link checking complete!");

	return {
		timestamp: new Date().toISOString(),
		totalFiles: files.length,
		totalLinks,
		brokenLinks,
		summary: {
			brokenCount: brokenLinks.length,
			validCount: totalLinks - brokenLinks.length,
		},
	};
}

async function getAllMarkdownFiles(): Promise<string[]> {
	const glob = new Glob(CONFIG.FILE_PATTERNS);
	const files = await Array.fromAsync(glob.scan({ cwd: CONFIG.DOCS_DIR }));
	return files.map((file) => join(CONFIG.DOCS_DIR, file));
}

async function getAllPublicAssets(): Promise<string[]> {
	const glob = new Glob("**/*");
	const files = await Array.fromAsync(glob.scan({ cwd: CONFIG.PUBLIC_DIR }));
	return files;
}

function buildFilePathMap(
	files: Array<string>,
	publicAssets: Array<string>,
): Set<string> {
	const pathMap = new Set<string>();

	const addPath = (path: string) => {
		if (path && typeof path === "string") pathMap.add(path);
	};

	for (const file of files) {
		const relativePath = relative(CONFIG.DOCS_DIR, file);

		addPath(relativePath);

		const withoutExt = relativePath.replace(CONFIG.MARKDOWN_EXTENSIONS, "");
		addPath(withoutExt);

		if (withoutExt.endsWith("/index"))
			addPath(withoutExt.replace("/index", ""));

		addPath(`/${withoutExt}`);
		if (withoutExt.endsWith("/index"))
			addPath(`/${withoutExt.replace("/index", "")}`);
	}

	for (const asset of publicAssets) addPath(`/${asset}`);

	return pathMap;
}

function extractLinksFromMarkdown(
	content: string,
): Array<{ link: string; line: number }> {
	const lines = content.split("\n");
	const links: Array<{ link: string; line: number }> = [];
	let inCodeBlock = false;

	for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
		const line = lines[lineIndex];
		const lineNumber = lineIndex + 1;

		// Toggle code block state
		if (line.trim().startsWith("```")) {
			inCodeBlock = !inCodeBlock;
			continue;
		}

		if (inCodeBlock) continue;

		const processedLine = line
			.split("`")
			.filter((_, index) => index % 2 === 0)
			.join("");

		links.push(...extractMarkdownLinks(processedLine, lineNumber));
		links.push(...extractHtmlLinks(processedLine, lineNumber));
	}

	return links;
}

function extractMarkdownLinks(
	line: string,
	lineNumber: number,
): Array<{ link: string; line: number }> {
	const regex = /\[([^\]]*)\]\(([^)]+)\)/g;
	return [...line.matchAll(regex)]
		.map(([, , url]) => ({ link: url, line: lineNumber }))
		.filter(({ link }) => isInternalLink(link));
}

function extractHtmlLinks(
	line: string,
	lineNumber: number,
): Array<{ link: string; line: number }> {
	const regex = /<a[^>]+href=["']([^"']+)["'][^>]*>/g;
	return [...line.matchAll(regex)]
		.map(([, url]) => ({ link: url, line: lineNumber }))
		.filter(({ link }) => isInternalLink(link));
}

function isInternalLink(url: string): boolean {
	return (
		!url.startsWith("http") &&
		!url.startsWith("mailto:") &&
		!url.startsWith("#")
	);
}

function validateLink(
	link: string,
	sourceFile: string,
	pathMap: Set<string>,
): string | null {
	const [linkPath] = link.split("#");
	if (!linkPath) return null; // Pure anchor link

	if (linkPath.startsWith("/")) return validateAbsolutePath(linkPath, pathMap);
	return validateRelativePath(linkPath, sourceFile, pathMap);
}

function validateAbsolutePath(
	linkPath: string,
	pathMap: Set<string>,
): string | null {
	const variations = [
		linkPath,
		linkPath.slice(1), // Remove leading slash
		linkPath.replace(/\/$/, ""), // Remove trailing slash
		linkPath
			.slice(1)
			.replace(/\/$/, ""), // Remove both
	];

	return variations.some((path) => pathMap.has(path))
		? null
		: `Absolute path not found: ${linkPath}`;
}

function validateRelativePath(
	linkPath: string,
	sourceFile: string,
	pathMap: Set<string>,
): string | null {
	const sourceDir = dirname(relative(CONFIG.DOCS_DIR, sourceFile));
	const resolvedPath = resolve(sourceDir, linkPath);
	const normalizedPath = relative(".", resolvedPath);

	const variations = [
		linkPath,
		normalizedPath,
		`/${normalizedPath}`,
		normalizedPath.replace(CONFIG.MARKDOWN_EXTENSIONS, ""),
		`/${normalizedPath.replace(CONFIG.MARKDOWN_EXTENSIONS, "")}`,
	];

	return variations.some((path) => pathMap.has(path))
		? null
		: `Relative path not found: ${linkPath} (resolved to: ${normalizedPath})`;
}

async function saveReport(report: LinkCheckReport) {
	try {
		await Bun.write(CONFIG.REPORT_PATH, JSON.stringify(report, null, 2));
		console.log(`\n📝 Report saved to: ${CONFIG.REPORT_PATH}`);
	} catch (error) {
		console.error(
			`\n⚠️  Warning: Failed to save report to ${CONFIG.REPORT_PATH}`,
		);
		console.error(error);
	}
}

function displayResults(report: LinkCheckReport) {
	LinkCheckReporter.printSummary(report);

	if (report.brokenLinks.length > 0)
		LinkCheckReporter.printBrokenLinks(report.brokenLinks);
	else console.log("\n✅ All links are valid!");
}

const LinkCheckReporter = {
	printSummary: (report: LinkCheckReport) => {
		console.log("\n📊 Link Check Summary:");
		console.log(`   📄 Files checked: ${report.totalFiles}`);
		console.log(`   🔗 Total links: ${report.totalLinks}`);
		console.log(`   ✅ Valid links: ${report.summary.validCount}`);
		console.log(`   ❌ Broken links: ${report.summary.brokenCount}`);
	},
	printBrokenLinks: (brokenLinks: Array<BrokenLink>) => {
		if (brokenLinks.length === 0) return;

		console.log("\n❌ Broken Links Found:\n");

		const byFile = brokenLinks.reduce(
			(acc, broken) => {
				if (!acc[broken.file]) acc[broken.file] = [];
				acc[broken.file].push(broken);
				return acc;
			},
			{} as Record<string, BrokenLink[]>,
		);

		for (const [file, links] of Object.entries(byFile)) {
			console.log(`📄 ${file}:`);
			for (const broken of links) {
				console.log(`   Line ${broken.line}: ${broken.link}`);
				console.log(`   └─ ${broken.reason}\n`);
			}
		}
	},
};