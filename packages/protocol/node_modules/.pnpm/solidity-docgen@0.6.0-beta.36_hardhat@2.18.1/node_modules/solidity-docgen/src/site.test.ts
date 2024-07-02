import test from './utils/test';
import path from 'path';
import { PageAssigner, Site, buildSite, pageAssigner, SiteConfig } from './site';

interface PageSummary {
  id: string;
  items: string[];
}

/**
 * @param files The name of the Solidity file whose contents should be considered.
 */
function testPages(title: string, opts: { files: string[], assign: PageAssigner, exclude?: string[] }, expected: PageSummary[]) {
  test(title, t => {
    const { files, assign, exclude = [] } = opts;
    const cfg: SiteConfig = {
      sourcesDir: 'test-contracts',
      exclude,
      pageExtension: '.md',
      pages: (i, f) => files.includes(path.parse(f.absolutePath).name) ? assign(i, f, cfg) : undefined,
    };
    const site = buildSite(t.context.build, cfg);
    const pages = site.pages.map(p => ({
      id: p.id,
      items: p.items.map(i => i.name),
    })).sort((a, b) => a.id.localeCompare(b.id));
    t.deepEqual(pages, expected);
  });
}

testPages('assign to single page',
  {
    files: ['S08_AB'],
    assign: pageAssigner.single,
  },
  [{ id: 'index.md', items: ['A', 'B'] }],
);

testPages('assign to item pages',
  {
    files: ['S08_AB'],
    assign: pageAssigner.items,
  },
  [
    { id: 'A.md', items: ['A'] },
    { id: 'B.md', items: ['B'] },
  ],
);

testPages('assign to file pages',
  {
    files: ['S08_AB', 'S08_C'],
    assign: pageAssigner.files,
  },
  [
    { id: 'S08_AB.md', items: ['A', 'B'] },
    { id: 'S08_C.md', items: ['C'] },
  ],
);

testPages('exclude',
  {
    files: ['S08_AB', 'S08_E0'],
    exclude: ['S08_E0.sol'],
    assign: pageAssigner.single,
  },
  [{ id: 'index.md', items: ['A', 'B'] }],
);
