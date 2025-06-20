# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is the **Reth documentation website** built with [Vocs](https://vocs.dev), a modern documentation framework. The site contains comprehensive documentation for Reth, the Ethereum execution client, including installation guides, CLI references, SDK documentation, and tutorials.

## Repository Structure

- **`docs/pages/`**: All documentation content in MDX format
  - `cli/`: Command-line interface documentation and references
  - `exex/`: Execution Extensions (ExEx) guides and examples
  - `installation/`: Installation and setup guides
  - `introduction/`: Introduction, benchmarks, and why-reth content
  - `jsonrpc/`: JSON-RPC API documentation
  - `run/`: Node running guides and configuration
  - `sdk/`: SDK documentation and examples
- **`docs/snippets/`**: Code examples and snippets used in documentation
- **`sidebar.ts`**: Navigation configuration
- **`vocs.config.ts`**: Vocs configuration file

## Essential Commands

```bash
# Install dependencies
bun install

# Start development server
bun run dev

# Build for production
bun run build

# Preview production build
bun run preview
```

## Development Workflow

### Content Organization

1. **MDX Files**: All content is written in MDX (Markdown + React components)
2. **Navigation**: Update `sidebar.ts` when adding new pages
3. **Code Examples**: Place reusable code snippets in `docs/snippets/`
4. **Assets**: Place images and static assets in `docs/public/`

### Adding New Documentation

1. Create new `.mdx` files in appropriate subdirectories under `docs/pages/`
2. Update `sidebar.ts` to include new pages in navigation
3. Use consistent heading structure and markdown formatting
4. Reference code examples from `docs/snippets/` when possible

### Code Examples and Snippets

- **Live Examples**: Use the snippets system to include actual runnable code
- **Rust Code**: Include cargo project examples in `docs/snippets/sources/`
- **CLI Examples**: Show actual command usage with expected outputs

### Configuration

- **Base Path**: Site deploys to `/reth` path (configured in `vocs.config.ts`)
- **Theme**: Custom accent colors for light/dark themes
- **Vite**: Uses Vite as the underlying build tool

### Content Guidelines

1. **Be Practical**: Focus on actionable guides and real-world examples
2. **Code First**: Show working code examples before explaining concepts
3. **Consistent Structure**: Follow existing page structures for consistency
4. **Cross-References**: Link between related pages and sections
5. **Keep Current**: Ensure documentation matches latest Reth features

### File Naming Conventions

- Use kebab-case for file and directory names
- Match URL structure to file structure
- Use descriptive names that reflect content purpose

### Common Tasks

**Adding a new CLI command documentation:**
1. Create `.mdx` file in `docs/pages/cli/reth/`
2. Add to sidebar navigation
3. Include usage examples and parameter descriptions

**Adding a new guide:**
1. Create `.mdx` file in appropriate category
2. Update sidebar with new entry
3. Include practical examples and next steps

**Updating code examples:**
1. Modify files in `docs/snippets/sources/`
2. Ensure examples compile and run correctly
3. Test that documentation references work properly

## Development Notes

- This is a TypeScript/React project using Vocs framework
- Content is primarily MDX with some TypeScript configuration
- Focus on clear, practical documentation that helps users succeed with Reth
- Maintain consistency with existing documentation style and structure