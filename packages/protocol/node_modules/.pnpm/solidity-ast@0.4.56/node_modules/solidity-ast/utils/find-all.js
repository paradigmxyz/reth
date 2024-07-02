const { isNodeType } = require('./is-node-type');
const finder = require('../finder.json');

const nextPropsCache = new Map();

function* findAll(nodeType, node, prune) {
  let cache;

  if (Array.isArray(nodeType)) {
    const cacheKey = JSON.stringify(nodeType);
    cache = nextPropsCache.get(cacheKey);
    if (!cache) {
      cache = {};
      nextPropsCache.set(cacheKey, cache);
    }
  }

  const queue = [];
  const push = node => queue.push({ node, props: getNextProps(nodeType, node.nodeType ?? '$other', cache) });

  push(node);

  for (let i = 0; i < queue.length; i++) {
    const { node, props } = queue[i];

    if (typeof node !== 'object' || (prune && prune(node))) {
      continue;
    }

    if (isNodeType(nodeType, node)) {
      yield node;
    }

    for (let j = 0; j < props.length; j++) {
      const member = node[props[j]];
      if (Array.isArray(member)) {
        for (const sub2 of member) {
          if (sub2) {
            push(sub2);
          }
        }
      } else if (member) {
        push(member);
      }
    }
  }
}

function getNextProps(wantedNodeTypes, currentNodeType, cache) {
  if (typeof wantedNodeTypes === 'string') {
    return finder[wantedNodeTypes]?.[currentNodeType] ?? [];
  }
  if (currentNodeType in cache) {
    return cache[currentNodeType];
  }
  const next = [];
  for (const wantedNodeType of wantedNodeTypes) {
    const wantedFinder = finder[wantedNodeType];
    if (wantedFinder && currentNodeType in wantedFinder) {
      for (const nextNodeType of wantedFinder[currentNodeType]) {
        if (!next.includes(nextNodeType)) {
          next.push(nextNodeType);
        }
      }
    }
  }
  cache[currentNodeType] = next;
  return next;
}

module.exports = {
  findAll,
};
