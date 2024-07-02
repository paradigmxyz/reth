import test from './utils/test';
import path from 'path';
import { buildSite, PageStructure, SiteConfig } from './site';
import { itemPartialName, render } from './render';
import { NodeType } from 'solidity-ast/node';
import { Templates } from './templates';

interface TestSpec extends Templates {
  collapseNewlines?: boolean;
}

/**
 * @param contracts The name of the Solidity file whose contents should be considered.
 */
function testRender(title: string, file: string, spec: TestSpec, expected: string) {
  const id = 'index.md';
  const cfg: SiteConfig = {
    sourcesDir: 'test-contracts',
    exclude: [],
    pageExtension: '.md',
    pages: (_, f) => path.parse(f.absolutePath).name === file ? id : undefined,
  };

  test(title, t => {
    const site = buildSite(t.context.build, cfg);
    const rendered = render(site, spec, spec.collapseNewlines);
    t.is(rendered.length, 1);
    t.is(rendered[0]!.contents, expected);
  });
}

testRender('static page',
  'S08_AB',
  { partials: { page: () => 'a page' } },
  'a page',
);

testRender('items',
  'S08_AB',
  { partials: { page: () => '{{#each items}}{{name}}, {{/each}}' } },
  'A, B, ',
);

testRender('partials',
  'S08_AB',
  {
    partials: {
      page: () => '{{#each items}}{{>part}}, {{/each}}',
      part: () => '{{name}}',
    },
  },
  'A, B, ',
);

testRender('item partial',
  'S08_AB',
  {
    partials: {
      page: () => '{{#each items}}{{>item}}, {{/each}}',
      contract: () => '{{name}}',
    },
  },
  'A, B, ',
);
