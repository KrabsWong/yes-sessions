// Run with Node for development only; the app executes this code in WKWebView.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const rust = readFileSync(new URL('../crates/yes-app/src/mermaid.rs', import.meta.url), 'utf8');
const script = rust.split('<script>\n')[1].split('</script>')[0]
  .replace('{encoded_source}', '"sequenceDiagram"')
  .replace('{labels}', '["Error", "Retry"]')
  .replace('{theme}', 'default')
  .replaceAll('{{', '{').replaceAll('}}', '}');

async function diagram(width, height, viewportWidth, viewportHeight) {
  const svg = { viewBox: { baseVal: { width, height } }, style: {} };
  const elements = Object.fromEntries(
    ['stage', 'diagram', 'fallback', 'retry', 'failure-message', 'source', 'error'].map(id => [id, {
      style: {}, clientWidth: viewportWidth, clientHeight: viewportHeight,
      addEventListener() {}, replaceChildren() {}, querySelector: () => svg,
      classList: { add() {}, remove() {} },
    }]),
  );
  let resize;
  const events = {};
  const context = vm.createContext({
    document: { getElementById: id => elements[id] },
    addEventListener: (name, callback) => { events[name] = callback; },
    ResizeObserver: class {
      constructor(callback) { resize = callback; }
      observe() {}
    },
    mermaid: { initialize() {}, render: async () => ({ svg: '<svg/>' }) },
  });
  vm.runInContext(script, context);
  await new Promise(setImmediate);
  return { elements, svg, resize, events, run: code => vm.runInContext(code, context) };
}

for (const [width, height] of [[540, 350], [2400, 300], [300, 2400]]) {
  const view = await diagram(width, height, 820, 500);
  const fitted = view.run('fitScale');
  assert.ok(width * fitted <= 788 && height * fitted <= 468);
  assert.ok(Math.abs(Math.max(width * fitted / 788, height * fitted / 468) - 1) < 1e-9);
  assert.equal(view.svg.style.width, `${width}px`);
  assert.equal(view.run('x + y'), 0);
  view.elements.stage.clientWidth = 1200;
  view.resize();
  assert.equal(view.run('fitScale'), Math.min(1168 / width, 468 / height));
  view.run('zoomBy(1.2)');
  const manual = view.elements.diagram.style.transform;
  view.elements.stage.clientWidth = 600;
  view.resize();
  assert.equal(view.elements.diagram.style.transform, manual);
  view.run('resetView()');
  assert.equal(view.run('fitScale'), Math.min(568 / width, 468 / height));
  assert.equal(view.run('scale'), 1);
  view.run('dragging=true');
  view.events.mousemove({ clientX: 30, clientY: 40 });
  view.elements.stage.clientWidth = 800;
  view.resize();
  assert.equal(view.run('x'), 30);
  assert.equal(view.run('y'), 40);
}

const deferred = await diagram(540, 350, 0, 0);
assert.equal(deferred.run('fitScale'), 1);
deferred.elements.stage.clientWidth = 820;
deferred.elements.stage.clientHeight = 500;
deferred.resize();
assert.equal(deferred.run('fitScale'), Math.min(788 / 540, 468 / 350));
console.log('Mermaid viewport tests passed');
