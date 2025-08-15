# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Next.js-based interactive visualization platform for the Reth Ethereum execution client. The project creates educational, interactive visualizations to help developers understand Reth's complex architecture through animations, simulations, and real-time state demonstrations.

## Architecture

### Tech Stack
- **Framework**: Next.js 15 with App Router and Turbopack
- **Language**: TypeScript 5
- **Styling**: Tailwind CSS v4
- **Animations**: Framer Motion
- **State Management**: Zustand
- **Code Display**: Monaco Editor
- **Flow Diagrams**: React Flow

### Project Structure
```
visualizations/
├── app/                     # Next.js app directory
│   ├── chapters/           # Chapter pages (each is a learning module)
│   │   ├── engine-api/     # Engine API flow visualization
│   │   ├── state-root/     # State root computation strategies
│   │   ├── trie/           # Trie architecture and navigation
│   │   └── transaction/    # Transaction lifecycle
│   ├── globals.css         # Global styles and Tailwind directives
│   └── page.tsx           # Landing page with chapter navigation
├── components/            # React components
│   ├── visualizations/    # Feature-specific visualization components
│   ├── ui/               # Reusable UI components
│   └── layouts/          # Layout components
├── lib/                  # Core utilities
│   ├── types/           # TypeScript type definitions
│   ├── constants/       # Shared constants and configurations
│   ├── hooks/           # Custom React hooks for simulations
│   └── utils.ts         # Utility functions
└── public/              # Static assets
```

## Development Commands

```bash
# Install dependencies
npm install

# Run development server with Turbopack (faster builds)
npm run dev

# Build for production
npm run build

# Start production server
npm run start

# Run linter
npm run lint

# Type checking
npx tsc --noEmit

# Format code (if prettier is configured)
npx prettier --write .
```

## Key Development Patterns

### Component Guidelines
- **Small, Focused Components**: Each component should have a single responsibility
- **Feature-Based Organization**: Group related components by feature area (e.g., `/components/visualizations/engine-api/`)
- **Shared UI Components**: Extract common patterns to `/components/ui/`
- **Type Safety**: Define all props with TypeScript interfaces
- **Animation First**: Use Framer Motion for all animations and transitions

### State Management
- Use Zustand stores for complex simulation state
- Keep component state local when possible
- Define stores in `/lib/stores/`

### Styling Approach
- Use Tailwind CSS classes for styling
- Apply Ethereum.org-inspired design patterns:
  - Gradient backgrounds with purple/blue tones
  - Glassmorphism effects for cards
  - Smooth transitions and hover states
- Use `clsx` and `tailwind-merge` for conditional classes

### Animation Patterns
```tsx
// Standard animation pattern
<motion.div
  initial={{ opacity: 0, y: 20 }}
  animate={{ opacity: 1, y: 0 }}
  transition={{ duration: 0.5 }}
>
  {/* Content */}
</motion.div>
```

### Code Display
Use Monaco Editor for displaying Rust code examples:
```tsx
import MonacoEditor from '@monaco-editor/react';

<MonacoEditor
  language="rust"
  theme="vs-dark"
  value={codeString}
  options={{ readOnly: true, minimap: { enabled: false } }}
/>
```

## Testing Approach

While no testing framework is currently configured, when adding tests:
1. Use Jest and React Testing Library for component tests
2. Test user interactions and state changes
3. Mock complex visualizations for unit tests
4. Add E2E tests for critical user flows

## Performance Considerations

1. **Code Splitting**: Each chapter is a separate route for automatic code splitting
2. **Animation Performance**: Use `transform` and `opacity` for animations (GPU-accelerated)
3. **State Updates**: Batch state updates in simulations to avoid excessive re-renders
4. **Image Optimization**: Use Next.js Image component for all images
5. **Turbopack**: Development server uses Turbopack for faster HMR

## Common Tasks

### Adding a New Chapter
1. Create new directory in `/app/chapters/[chapter-name]/`
2. Add `page.tsx` with the chapter component
3. Create visualization components in `/components/visualizations/[chapter-name]/`
4. Add chapter to navigation in `/app/page.tsx`
5. Define types in `/lib/types/[chapter-name].ts`

### Creating Interactive Simulations
1. Define simulation state with Zustand store
2. Create visualization component with controls
3. Use `requestAnimationFrame` for smooth animations
4. Add metrics display for performance indicators
5. Include reset and step-through controls

### Implementing Flow Diagrams
Use React Flow for node-based visualizations:
```tsx
import ReactFlow from 'reactflow';
import 'reactflow/dist/style.css';
```

## Design System

### Colors (Ethereum.org inspired)
- Primary: Purple/Blue gradients
- Background: Dark theme with subtle gradients
- Text: High contrast white/gray on dark backgrounds
- Accents: Bright colors for interactive elements

### Typography
- Headers: Bold, large text with gradient effects
- Body: Clear, readable text with proper line height
- Code: Monospace font for code displays

### Components
- Cards with glassmorphism effects
- Buttons with hover animations
- Metric displays with live updates
- Progress indicators for multi-step processes

## Deployment

The app is configured for Vercel deployment:
1. Push to main branch triggers automatic deployment
2. Preview deployments for pull requests
3. Environment variables configured in Vercel dashboard

## Important Notes

- This is an educational visualization tool, not production Reth code
- Focus on clarity and interactivity over performance
- Animations should enhance understanding, not distract
- Keep mobile responsiveness in mind for all visualizations
- Maintain consistency with Ethereum.org design language