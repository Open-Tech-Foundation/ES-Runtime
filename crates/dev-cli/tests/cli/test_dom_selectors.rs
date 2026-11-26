//! `esdev test`'s DOM: selectors and the styles they cascade.

use super::*;

#[test]
fn test_dom_cascades_stylesheets_into_computed_styles() {
    let dir = build_dir("t_test_dom_cascade");
    write_in(
        &dir,
        "cascade.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         function sheet(text) {\n\
           const element = document.createElement('style');\n\
           element.textContent = text;\n\
           document.head.append(element);\n\
           return element;\n\
         }\n\
         // One realm per file, so each case starts from an empty document.\n\
         function reset() {\n\
           document.head.replaceChildren();\n\
           document.body.replaceChildren();\n\
           document.adoptedStyleSheets = [];\n\
         }\n\
         test('origin, importance, specificity and order decide the winner', () => {\n\
           reset();\n\
           sheet('div { color: one } .a { color: two } #b { color: three } .a { color: four } .imp { color: five !important }');\n\
           const element = document.createElement('div');\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).color, 'one');\n\
           element.className = 'a';\n\
           assertEquals(getComputedStyle(element).color, 'four');\n\
           element.id = 'b';\n\
           assertEquals(getComputedStyle(element).color, 'three');\n\
           element.style.color = 'inline';\n\
           assertEquals(getComputedStyle(element).color, 'inline');\n\
           element.classList.add('imp');\n\
           assertEquals(getComputedStyle(element).color, 'five');\n\
           element.style.setProperty('color', 'inline', 'important');\n\
           assertEquals(getComputedStyle(element).color, 'inline');\n\
           assertThrows(() => { getComputedStyle(element).color = 'x'; }, TypeError);\n\
         });\n\
         test('inheritance and the user-agent defaults fill the rest in', () => {\n\
           reset();\n\
           sheet('.parent { color: purple; border-color: red; --brand: cyan }');\n\
           const parent = document.createElement('div');\n\
           parent.className = 'parent';\n\
           const child = document.createElement('span');\n\
           parent.append(child);\n\
           document.body.append(parent);\n\
           const computed = getComputedStyle(child);\n\
           assertEquals([computed.color, computed.getPropertyValue('border-color'), computed.getPropertyValue('--brand')], ['rgb(128, 0, 128)', '', 'cyan']);\n\
           assertEquals([computed.display, computed.fontWeight, computed.visibility, computed.textAlign], ['inline', '400', 'visible', 'start']);\n\
           assertEquals(getComputedStyle(parent).display, 'block');\n\
           const strong = document.createElement('strong');\n\
           document.body.append(strong);\n\
           assertEquals(getComputedStyle(strong).fontWeight, '700');\n\
           const hidden = document.createElement('div');\n\
           hidden.setAttribute('hidden', '');\n\
           document.body.append(hidden);\n\
           assertEquals(getComputedStyle(hidden).display, 'none');\n\
         });\n\
         test('a media query is answered from the declared viewport', () => {\n\
           reset();\n\
           sheet('@media (min-width: 100px) { .m { color: wide } } @media (min-width: 99999px) { .m { color: wider } } @media print { .m { color: printed } }');\n\
           const element = document.createElement('p');\n\
           element.className = 'm';\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).color, 'wide');\n\
           window.innerWidth = 50;\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
           window.innerWidth = 1024;\n\
           assertEquals(getComputedStyle(element).color, 'wide');\n\
         });\n\
         test('a constructed sheet applies while it is adopted', () => {\n\
           reset();\n\
           const constructed = new CSSStyleSheet();\n\
           constructed.replaceSync('.c { color: adopted }');\n\
           const element = document.createElement('div');\n\
           element.className = 'c';\n\
           document.body.append(element);\n\
           assertEquals([constructed.cssRules.length, constructed.cssRules[0].selectorText], [1, '.c']);\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
           document.adoptedStyleSheets = [constructed];\n\
           assertEquals(getComputedStyle(element).color, 'adopted');\n\
           document.adoptedStyleSheets = [];\n\
           assertEquals(getComputedStyle(element).color, 'rgb(0, 0, 0)');\n\
         });\n\
         test('a shadow root is styled by its own sheets only', () => {\n\
           reset();\n\
           const host = document.createElement('div');\n\
           document.body.append(host);\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           root.innerHTML = '<style>.in { color: maroon }</style><p class=in>x</p>';\n\
           const inside = root.lastElementChild;\n\
           const outside = document.createElement('p');\n\
           outside.className = 'in';\n\
           document.body.append(outside);\n\
           assertEquals([getComputedStyle(inside).color, getComputedStyle(outside).color], ['rgb(128, 0, 0)', 'rgb(0, 0, 0)']);\n\
           const scoped = new CSSStyleSheet();\n\
           scoped.replaceSync('.in { font-weight: 700 }');\n\
           root.adoptedStyleSheets = [scoped];\n\
           assertEquals(getComputedStyle(inside).fontWeight, '700');\n\
         });\n\
         test('nesting resolves through :is() and reports its own rules', () => {\n\
           reset();\n\
           const element = sheet('.card { color: olive; & a { color: coral } b { color: lime } }');\n\
           const card = document.createElement('div');\n\
           card.className = 'card';\n\
           card.innerHTML = '<a>l</a><b>bold</b>';\n\
           document.body.append(card);\n\
           assertEquals(getComputedStyle(card).color, 'rgb(128, 128, 0)');\n\
           assertEquals(getComputedStyle(card.firstElementChild).color, 'rgb(255, 127, 80)');\n\
           assertEquals(getComputedStyle(card.lastElementChild).color, 'rgb(0, 255, 0)');\n\
           assertEquals(Array.from(element.sheet.cssRules, rule => rule.selectorText), ['.card', ':is(.card) a', ':is(.card) b']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom cascade");
    assert!(
        ran.status.success(),
        "DOM cascade test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_count_an_of_list_and_follow_custom_definitions() {
    let dir = build_dir("t_test_dom_selector_of_and_defined");
    write_in(
        &dir,
        "selectors.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('nth-child counts only the siblings in its of list', () => {\n\
           document.body.innerHTML = \"<ul><li class='x'>1</li><li>2</li><li class='x'>3</li><li class='x'>4</li><li>5</li><li class='x'>6</li></ul>\";\n\
           const list = document.body.firstElementChild;\n\
           const texts = selector => Array.from(list.querySelectorAll(selector), item => item.textContent);\n\
           assertEquals(texts('li:nth-child(2n + 1 of .x)'), ['1', '4']);\n\
           assertEquals(texts('li:nth-child(odd of .x)'), ['1', '4']);\n\
           assertEquals(texts('li:nth-last-child(1 of .x)'), ['6']);\n\
           assertEquals(texts('li:nth-child(2n + 1)'), ['1', '3', '5']);\n\
           assertEquals(texts('li:nth-child(1 of .x, :not(.x))'), ['1']);\n\
         });\n\
         test('an of list is refused where it does not belong', () => {\n\
           const name = callback => { try { callback(); return 'ok'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => document.querySelectorAll('li:nth-child(of .x)')), 'SyntaxError');\n\
           assertEquals(name(() => document.querySelectorAll('li:nth-of-type(2n of .x)')), 'SyntaxError');\n\
           assertEquals(name(() => document.querySelectorAll(':defined(x)')), 'SyntaxError');\n\
         });\n\
         test(':defined answers before and after the definition', () => {\n\
           const host = document.createElement('div');\n\
           document.body.append(host);\n\
           host.innerHTML = '<my-widget></my-widget><p></p><other-thing></other-thing>';\n\
           const names = selector => Array.from(host.querySelectorAll(selector), element => element.localName);\n\
           assertEquals([names(':defined'), host.firstElementChild.matches(':defined')], [['p'], false]);\n\
           customElements.define('my-widget', class extends HTMLElement {});\n\
           assertEquals([names(':defined'), host.firstElementChild.matches(':defined')], [['my-widget', 'p'], true]);\n\
           assertEquals(names(':not(:defined)'), ['other-thing']);\n\
           assertEquals([document.createElement('my-widget').matches(':defined'), document.createElement('late-one').matches(':defined')], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom of-list and defined selectors");
    assert!(
        ran.status.success(),
        "DOM selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_strict_compounds_and_combinators() {
    let dir = build_dir("t_test_dom_selectors");
    write_in(
        &dir,
        "selectors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('selector core', () => {\n\
           document.body.innerHTML = '<section id=panel><article class=\"card selected\" data-tags=\"new hot\"><span class=label data-state=\"Ready-item\">one</span></article><article class=card data-tags=old>two</article></section>';\n\
           const panel = document.querySelector('#panel');\n\
           const label = panel.querySelector('article.card.selected > span.label[data-state|=ready i]');\n\
           assertEquals(label.textContent, 'one');\n\
           assertEquals(document.querySelectorAll('#panel > article').length, 2);\n\
           assertEquals(document.querySelector('#panel > article + article').textContent, 'two');\n\
           assertEquals(document.querySelectorAll('#panel > article ~ article').length, 1);\n\
           assertEquals(document.querySelector('[data-tags~=hot]').textContent, 'one');\n\
           assertEquals(document.querySelector('[data-state^=Ready][data-state$=item][data-state*=dy]').textContent, 'one');\n\
           assertEquals(document.querySelector('.missing, .label'), label);\n\
           assertEquals(label.matches('span.label[data-state=Ready-item]'), true);\n\
           assertEquals(label.closest('section#panel'), panel);\n\
           assertThrows(() => document.querySelector(':nonsense'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom selectors");
    assert!(
        ran.status.success(),
        "DOM selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_logical_pseudo_classes_and_nested_lists() {
    let dir = build_dir("t_test_dom_selector_logical");
    write_in(
        &dir,
        "logical.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('is where and not compose selector lists', () => {\n\
           document.body.innerHTML = '<main><button class=\"primary selected\">save</button><a class=link href=\"/docs\">docs</a><p class=muted>note</p></main>';
           const [button, link, note] = document.querySelectorAll('main > *');\n\
           assertEquals(document.querySelectorAll('main > :is(button, a.link)').length, 2);\n\
           assertEquals(document.querySelectorAll('main > :where(button.primary, a[href])').length, 2);\n\
           assertEquals(Array.from(document.querySelectorAll('main > :not(:is(.muted, a))')), [button]);\n\
           assertEquals(button.matches(':not(.muted):is(.primary, .secondary)'), true);\n\
           assertEquals(link.matches(':where(.link, .missing)'), true); assertEquals(note.matches(':not(button, a)'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM logical selector test");
    assert!(
        ran.status.success(),
        "logical selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_root_empty_and_child_positions() {
    let dir = build_dir("t_test_dom_selector_structure");
    write_in(
        &dir,
        "structure.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('root empty and child positions count elements', () => {\n\
           document.body.innerHTML = '<section id=parent><i id=one></i>text<b id=two></b><em id=three></em></section><aside id=empty></aside>';\n\
           const parent = document.querySelector('#parent'); const [one, two, three] = parent.children; const empty = document.querySelector('#empty');\n\
           assertEquals(document.documentElement.matches(':root'), true); assertEquals(parent.matches(':root'), false);\n\
           assertEquals(empty.matches(':empty'), true); assertEquals(parent.matches(':empty'), false);\n\
           assertEquals(Array.from(parent.querySelectorAll(':first-child')), [one]);\n\
           assertEquals(Array.from(parent.querySelectorAll(':last-child')), [three]);\n\
           assertEquals(two.matches(':only-child'), false);\n\
           parent.innerHTML = '<strong id=single></strong>'; assertEquals(parent.firstChild.matches(':only-child'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM structural selector test");
    assert!(
        ran.status.success(),
        "structural selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_selector_follows_focus_transitions() {
    let dir = build_dir("t_test_dom_selector_focus");
    write_in(
        &dir,
        "focus.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('focus selection follows the active element', () => {\n\
           const first = document.createElement('input'); const second = document.createElement('button'); document.body.append(first, second);\n\
           assertEquals(document.querySelector(':focus'), document.body);\n\
           first.focus(); assertEquals(document.querySelector(':focus'), first); assertEquals(second.matches(':focus'), false);\n\
           second.focus(); assertEquals(document.querySelector(':focus'), second);\n\
           second.blur(); assertEquals(document.querySelector(':focus'), document.body);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus selector test");
    assert!(
        ran.status.success(),
        "focus selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_links_without_treating_plain_anchors_as_links() {
    let dir = build_dir("t_test_dom_selector_links");
    write_in(
        &dir,
        "links.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('link state is driven by href', () => {\n\
           const anchor = document.createElement('a'); const area = document.createElement('area'); const plain = document.createElement('a');\n\
           anchor.setAttribute('href', '/docs'); area.setAttribute('href', '/map'); document.body.append(anchor, area, plain);\n\
           assertEquals(Array.from(document.querySelectorAll(':link')), [anchor, area]);\n\
           anchor.removeAttribute('href'); assertEquals(Array.from(document.querySelectorAll(':link')), [area]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM link selector test");
    assert!(
        ran.status.success(),
        "link selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_scope_selector_respects_the_query_root() {
    let dir = build_dir("t_test_dom_selector_scope");
    write_in(
        &dir,
        "scope.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('scope includes the element root only when requested', () => {\n\
           const root = document.createElement('div'); const child = document.createElement('div'); const grandchild = document.createElement('span');\n\
           child.className = 'child'; grandchild.className = 'grandchild'; child.appendChild(grandchild); root.appendChild(child); document.body.appendChild(root);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope')), [root]);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope > .child')), [child]);\n\
           assertEquals(Array.from(root.querySelectorAll(':scope .grandchild')), [grandchild]);\n\
           assertEquals(Array.from(root.querySelectorAll('div')), [child]);\n\
           assertEquals(root.matches(':scope'), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM scope selector test");
    assert!(
        ran.status.success(),
        "scope selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selector_node_lists_support_for_each() {
    let dir = build_dir("t_test_dom_selector_node_list_for_each");
    write_in(
        &dir,
        "selector-list.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('selector node lists expose indexed forEach iteration', () => {\n\
           const root = document.createElement('div'); root.innerHTML = '<i>one</i><b>two</b>'; document.body.appendChild(root);\n\
           const list = root.querySelectorAll('*'); const seen = [];\n\
           list.forEach(function(node, index, received) { seen.push([this.prefix, node.localName, index, received === list]); }, { prefix: 'node' });\n\
           assertEquals(seen, [['node', 'i', 0, true], ['node', 'b', 1, true]]);\n\
           assertThrows(() => list.forEach(null), TypeError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM selector NodeList forEach test");
    assert!(
        ran.status.success(),
        "selector NodeList forEach test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_inline_styles_track_the_style_attribute() {
    let dir = build_dir("t_test_dom_css");
    write_in(
        &dir,
        "styles.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('properties and attributes stay synchronized', () => {\n\
           const element = document.createElement('div');\n\
           element.style.width = '10px';\n\
           element.style.setProperty('--tone', 'red', 'important');\n\
           assertEquals(element.getAttribute('style'), 'width: 10px; --tone: red !important;');\n\
           assertEquals(element.style.length, 2);\n\
           assertEquals(element.style[0], 'width');\n\
           assertEquals(element.style.getPropertyPriority('--tone'), 'important');\n\
           assertEquals(element.style.removeProperty('width'), '10px');\n\
           assertEquals(element.style.width, '');\n\
           element.setAttribute('style', 'background-color: rgb(1, 2, 3); padding: 1px !important;');\n\
           assertEquals(element.style.backgroundColor, 'rgb(1, 2, 3)');\n\
           assertEquals(element.style.padding, '1px');\n\
           assertEquals(element.style.getPropertyPriority('padding'), 'important');\n\
         });\n\
         test('invalid declarations fail loudly', () => {\n\
           const element = document.createElement('div');\n\
           assertThrows(() => { element.style.cssText = 'width'; }, SyntaxError);\n\
           assertThrows(() => element.style.setProperty('width', '1px', 'urgent'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom styles");
    assert!(
        ran.status.success(),
        "DOM style test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_style_properties_and_progress_values_are_observable() {
    let dir = build_dir("t_test_dom_style_and_progress");
    write_in(
        &dir,
        "style-progress.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('style property checks and common HTML reflectors work', () => {
             const div = document.createElement('div');
             div.style.opacity = 0; div.style.animationIterationCount = 2.5;
             const progress = document.createElement('progress');
             const table = document.createElement('table');
             progress.value = 0; table.border = false;
             assertEquals(['opacity' in div.style, 'animationIterationCount' in div.style, div.style.opacity, div.style.animationIterationCount, progress instanceof HTMLProgressElement, progress.value, table.getAttribute('border')], [true, true, '0', '2.5', true, 0, 'false']);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn style and progress DOM test");
    assert!(
        ran.status.success(),
        "style and progress DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
