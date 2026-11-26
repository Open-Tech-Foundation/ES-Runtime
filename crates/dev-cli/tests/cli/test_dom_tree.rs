//! `esdev test`'s DOM: parsing and serializing, the tree and its mutation, ranges, shadow roots and custom elements.

use super::*;

#[test]
fn test_dom_parser_builds_whole_documents_from_html_only() {
    let dir = build_dir("t_test_dom_domparser");
    write_in(
        &dir,
        "domparser.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('a bare document gets a synthesized html, head and body', () => {\n\
           const parsed = new DOMParser().parseFromString('<p>one</p><style>a{}</style>', 'text/html');\n\
           assertEquals(parsed.doctype, null);\n\
           assertEquals(parsed.documentElement.tagName, 'HTML');\n\
           assertEquals([parsed.head.innerHTML, parsed.body.innerHTML], ['<style>a{}</style>', '<p>one</p>']);\n\
           assertEquals(parsed.firstElementChild === parsed.documentElement, true);\n\
         });\n\
         test('a document that supplies its own structure keeps it', () => {\n\
           const source = '<!doctype html><html><head><title>T</title></head><body><main id=\"x\">y</main></body></html>';\n\
           const parsed = new DOMParser().parseFromString(source, 'text/html');\n\
           assertEquals([parsed.doctype.name, parsed.title, parsed.getElementById('x').textContent], ['html', 'T', 'y']);\n\
           assertEquals([parsed !== document, parsed.body.ownerDocument === parsed], [true, true]);\n\
         });\n\
         test('XML is refused by name and malformed HTML by the parser', () => {\n\
           const parser = new DOMParser();\n\
           const name = callback => { try { callback(); return 'parsed'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => parser.parseFromString('<p/>', 'text/xml')), 'NotSupportedError');\n\
           assertEquals(name(() => parser.parseFromString('<p></p>', 'text/plain')), 'TypeError');\n\
           assertEquals(name(() => parser.parseFromString('<p><i>unclosed', 'text/html')), 'SyntaxError');\n\
           assertEquals(name(() => parser.parseFromString('<p>x</p><!doctype html>', 'text/html')), 'SyntaxError');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom DOMParser");
    assert!(
        ran.status.success(),
        "DOM DOMParser test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_inserts_adjacent_markup_nodes_and_text() {
    let dir = build_dir("t_test_dom_insert_adjacent");
    write_in(
        &dir,
        "adjacent.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('the four insertion positions land where they say', () => {\n\
           const root = document.createElement('main');\n\
           const target = document.createElement('p');\n\
           root.append(target);\n\
           document.body.append(root);\n\
           target.insertAdjacentHTML('beforebegin', '<i>bb</i>');\n\
           target.insertAdjacentHTML('afterbegin', '<b>ab</b>');\n\
           target.insertAdjacentHTML('beforeend', '<u>be</u>');\n\
           target.insertAdjacentHTML('afterend', '<s>ae</s>');\n\
           assertEquals(root.innerHTML, '<i>bb</i><p><b>ab</b><u>be</u></p><s>ae</s>');\n\
           const returned = target.insertAdjacentElement('AfterBegin', document.createElement('em'));\n\
           target.insertAdjacentText('beforeend', 'text&');\n\
           assertEquals([returned.localName, target.innerHTML], ['em', '<em></em><b>ab</b><u>be</u>text&amp;']);\n\
         });\n\
         test('a position with no parent or no name is refused', () => {\n\
           const orphan = document.createElement('div');\n\
           const errors = [];\n\
           const name = callback => { try { callback(); return 'inserted'; } catch (error) { return error.name; } };\n\
           errors.push(name(() => orphan.insertAdjacentHTML('beforebegin', 'x')));\n\
           errors.push(name(() => document.documentElement.insertAdjacentHTML('afterend', 'x')));\n\
           errors.push(name(() => orphan.insertAdjacentHTML('nowhere', 'x')));\n\
           errors.push(name(() => orphan.insertAdjacentElement('beforeend', 'nope')));\n\
           assertEquals(errors, ['NoModificationAllowedError', 'NoModificationAllowedError', 'SyntaxError', 'TypeError']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom insertAdjacent");
    assert!(
        ran.status.success(),
        "DOM adjacent-insertion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_declarative_shadow_roots_attach_only_through_set_html_unsafe() {
    let dir = build_dir("t_test_dom_declarative_shadow");
    write_in(
        &dir,
        "declarative.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('innerHTML keeps the template, setHTMLUnsafe attaches it', () => {\n\
           const kept = document.createElement('div');\n\
           kept.innerHTML = '<span><template shadowrootmode=\"open\"><i>inner</i></template></span>';\n\
           assertEquals([kept.firstElementChild.shadowRoot, kept.firstElementChild.firstElementChild.localName], [null, 'template']);\n\
           const attached = document.createElement('div');\n\
           attached.setHTMLUnsafe('<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>');\n\
           const host = attached.firstElementChild;\n\
           assertEquals([host.shadowRoot.mode, host.shadowRoot.serializable, host.shadowRoot.innerHTML, host.textContent], ['open', true, '<i>inner</i>', 'light']);\n\
         });\n\
         test('getHTML serializes a shadow root only when asked', () => {\n\
           const root = document.createElement('div');\n\
           root.setHTMLUnsafe('<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>');\n\
           const shadow = root.firstElementChild.shadowRoot;\n\
           const serialized = '<span><template shadowrootmode=\"open\" shadowrootserializable=\"\"><i>inner</i></template>light</span>';\n\
           assertEquals(root.getHTML(), '<span>light</span>');\n\
           assertEquals(root.innerHTML, '<span>light</span>');\n\
           assertEquals(root.getHTML({ serializableShadowRoots: true }), serialized);\n\
           assertEquals(root.getHTML({ shadowRoots: [shadow] }), serialized);\n\
           const closed = document.createElement('div');\n\
           closed.setHTMLUnsafe('<p><template shadowrootmode=\"closed\"><b>x</b></template></p>');\n\
           assertEquals([closed.firstElementChild.shadowRoot, closed.getHTML({ serializableShadowRoots: true })], [null, '<p></p>']);\n\
         });\n\
         test('nested declarative roots attach, a repeated one is dropped', () => {\n\
           const nested = document.createElement('div');\n\
           // Only an element that may host a root is used: `<a>` and `<b>` may\n\
           // not, here or in a browser.\n\
           nested.setHTMLUnsafe('<section><template shadowrootmode=\"open\"><div><template shadowrootmode=\"open\"><u>deep</u></template></div></template></section>');\n\
           const inner = nested.firstElementChild.shadowRoot.firstElementChild;\n\
           assertEquals([inner.localName, inner.shadowRoot.innerHTML], ['div', '<u>deep</u>']);\n\
           const twice = document.createElement('div');\n\
           twice.setHTMLUnsafe('<span><template shadowrootmode=\"open\"><i>1</i></template><template shadowrootmode=\"open\"><i>2</i></template></span>');\n\
           assertEquals([twice.firstElementChild.shadowRoot.innerHTML, twice.firstElementChild.childNodes.length], ['<i>1</i>', 0]);\n\
         });\n\
         test('a manual-assignment root holds only what it was assigned', () => {\n\
           const host = document.createElement('div');\n\
           const first = document.createElement('i');\n\
           const second = document.createElement('b');\n\
           host.append(first, second);\n\
           document.body.append(host);\n\
           const root = host.attachShadow({ mode: 'open', slotAssignment: 'manual' });\n\
           root.innerHTML = '<slot></slot>';\n\
           const slot = root.firstElementChild;\n\
           assertEquals([root.slotAssignment, slot.assignedNodes().length], ['manual', 0]);\n\
           slot.assign(second, document.createElement('u'));\n\
           assertEquals(slot.assignedNodes().map(node => node.localName), ['b']);\n\
           const named = document.createElement('div');\n\
           named.append(document.createElement('i'));\n\
           const namedRoot = named.attachShadow({ mode: 'open' });\n\
           namedRoot.innerHTML = '<slot></slot>';\n\
           assertEquals([namedRoot.slotAssignment, namedRoot.firstElementChild.assignedNodes().length, namedRoot.clonable], ['named', 1, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom declarative shadow roots");
    assert!(
        ran.status.success(),
        "DOM declarative shadow-root test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_installs_a_fresh_document_and_uses_the_strict_parser() {
    let dir = build_dir("t_test_dom");
    write_in(
        &dir,
        "dom.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('tree and parser', () => {\n\
           assertEquals(document.body.localName, 'body');\n\
           document.body.innerHTML = '<p id=x>hello<!-- note --></p>';\n\
           assertEquals(document.body.firstChild.textContent, 'hello');\n\
           assertEquals(document.body.innerHTML, '<p id=\"x\">hello<!-- note --></p>');\n\
           document.body.innerHTML = '<p>';\n\
           assertEquals(document.body.innerHTML, '<p></p>');\n\
           assertThrows(() => { document.body.innerHTML = '<div>'; }, SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    assert!(
        ran.status.success(),
        "DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_serializes_text_attributes_raw_text_voids_and_replacements() {
    let dir = build_dir("t_test_dom_serialize");
    write_in(
        &dir,
        "serialize.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('serialization contexts', () => {\n\
           const text = document.createTextNode('&<>');\n\
           const image = document.createElement('img');\n\
           image.setAttribute('title', 'a&\\\"b\\u00a0');\n\
           const script = document.createElement('script');\n\
           script.textContent = 'a < b && c > d';\n\
           document.body.append(text, image, script);\n\
           assertEquals(document.body.innerHTML, '&amp;&lt;&gt;<img title=\"a&amp;&quot;b&nbsp;\"><script>a < b && c > d</script>');\n\
         });\n\
         test('outerHTML replacement', () => {\n\
           document.body.innerHTML = '<p>old</p><br>';\n\
           document.body.firstChild.outerHTML = '<em>new</em><!--tail-->';
           assertEquals(document.body.innerHTML, '<em>new</em><!--tail--><br>');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom serializer cases");
    assert!(
        ran.status.success(),
        "DOM serializer cases did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_attributes_are_exposed_by_an_element_prototype_getter() {
    let dir = build_dir("t_test_dom_attributes_accessor");
    write_in(
        &dir,
        "attributes-accessor.test.mjs",
        "import { test, expect, mock } from 'runtime:test';\n\
         test('attributes has one inherited, spyable named-node-map getter', () => {\n\
           const element = document.createElement('article'); element.setAttribute('data-id', 'one');\n\
           const read = mock.spyOn(Element.prototype, 'attributes', 'get');\n\
           expect(Object.hasOwn(element, 'attributes')).toBe(false);\n\
           expect(element.attributes.item(0).value).toBe('one');\n\
           expect(read).toHaveBeenCalledOnce();\n\
           read.mockRestore();\n\
           expect(element.attributes.getNamedItem('data-id').value).toBe('one');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom attributes accessor");
    assert!(
        ran.status.success(),
        "DOM attributes-accessor test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_replace_children_coerces_strings_and_moves_fragments() {
    let dir = build_dir("t_test_dom_replace_children");
    write_in(
        &dir,
        "replace-children.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('replaceChildren coerces strings and empties fragments', () => {
  const host = document.createElement('div'); host.append('old', document.createElement('i'));
  const fragment = document.createDocumentFragment(); fragment.append(document.createElement('b'), 'tail');
  host.replaceChildren('head', fragment);
  assertEquals([host.innerHTML, fragment.childNodes.length], ['head<b></b>tail', 0]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM replaceChildren test");
    assert!(
        ran.status.success(),
        "replaceChildren test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_element_child_accessors_skip_non_elements() {
    let dir = build_dir("t_test_dom_element_child_accessors");
    write_in(
        &dir,
        "element-children.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('element child accessors skip text and comments', () => {
  const parent = document.createElement('div'); const first = document.createElement('i'); const last = document.createElement('b');
  parent.append('before', first, document.createComment('note'), last, 'after');
  assertEquals([parent.childElementCount, parent.firstElementChild, parent.lastElementChild], [2, first, last]);
  assertEquals(Array.prototype.slice.call(parent.childNodes), Array.from(parent.childNodes));
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM element child accessors test");
    assert!(
        ran.status.success(),
        "element child accessors test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_element_sibling_accessors_follow_tree_mutations() {
    let dir = build_dir("t_test_dom_element_sibling_accessors");
    write_in(
        &dir,
        "element-siblings.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('element sibling accessors follow insertions and removals', () => {
  const parent = document.createElement('div'); const first = document.createElement('i'); const middle = document.createElement('b'); const last = document.createElement('em');
  parent.append(first, 'text', middle, document.createComment('note'), last);
  assertEquals([middle.previousElementSibling, middle.nextElementSibling], [first, last]);
  middle.remove(); assertEquals([first.nextElementSibling, last.previousElementSibling], [last, first]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM element sibling accessors test");
    assert!(
        ran.status.success(),
        "element sibling accessors test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_tracks_and_validates_boundary_points() {
    let dir = build_dir("t_test_dom_range_boundaries");
    write_in(
        &dir,
        "boundaries.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('boundary setters collapse and reject invalid offsets', () => {\n\
           const parent = document.createElement('div'); const first = document.createElement('i'); const second = document.createElement('b'); parent.append(first, second); document.body.appendChild(parent);\n\
           const range = document.createRange(); assertEquals(range instanceof Range, true); assertEquals(range.collapsed, true);\n\
           range.setStart(parent, 1); range.setEnd(parent, 2); assertEquals([range.startOffset, range.endOffset, range.collapsed], [1, 2, false]);\n\
           range.setStart(parent, 2); assertEquals([range.startOffset, range.endOffset, range.collapsed], [2, 2, true]);\n\
           range.setEnd(parent, 0); assertEquals([range.startOffset, range.endOffset, range.collapsed], [0, 0, true]);\n\
           range.setStartBefore(second); range.setEndAfter(second); assertEquals([range.startOffset, range.endOffset], [1, 2]);\n\
           assertThrows(() => range.setStart(parent, 3), DOMException); assertThrows(() => range.setEnd(parent, -1), DOMException);\n\
           const other = new Document().createElement('div'); range.setStart(other, 0);\n\
           assertEquals([range.startContainer, range.endContainer, range.collapsed], [other, other, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range boundary test");
    assert!(
        ran.status.success(),
        "range boundary test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_extracts_text_across_tree_boundaries() {
    let dir = build_dir("t_test_dom_range_text");
    write_in(
        &dir,
        "text.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('range text walks text nodes between boundaries', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p');\n\
           const first = paragraph.firstChild; const last = paragraph.lastChild; const range = document.createRange();\n\
           range.setStart(first, 1); range.setEnd(last, 2); assertEquals(range.toString(), 'ne two t');\n\
           const selected = document.createRange(); selected.selectNode(paragraph.querySelector('b'));\n\
           assertEquals(selected.toString(), 'two'); assertEquals(selected.commonAncestorContainer, paragraph);\n\
           assertEquals(range.compareBoundaryPoints(Range.START_TO_START, selected), -1);\n\
           assertEquals(range.compareBoundaryPoints(Range.END_TO_END, selected), 1);\n\
           range.collapse(true); assertEquals(range.toString(), '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range text test");
    assert!(
        ran.status.success(),
        "range text test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_creates_contextual_fragments() {
    let dir = build_dir("t_test_dom_range_fragment");
    write_in(
        &dir,
        "fragment.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('contextual fragments use the start container context', () => {\n\
           const host = document.createElement('section'); const range = document.createRange(); document.body.appendChild(host); range.selectNodeContents(host);\n\
           const fragment = range.createContextualFragment('<em class=note>new</em><!--tail-->');\n\
           assertEquals(fragment.nodeType, Node.DOCUMENT_FRAGMENT_NODE); assertEquals(fragment.firstChild.localName, 'em'); assertEquals(fragment.firstChild.className, 'note');\n\
           host.appendChild(fragment); assertEquals(host.innerHTML, '<em class=\"note\">new</em><!--tail-->');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range fragment test");
    assert!(
        ran.status.success(),
        "range fragment test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_inserts_nodes_at_text_and_element_boundaries() {
    let dir = build_dir("t_test_dom_range_insert");
    write_in(
        &dir,
        "insert.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('insertNode splits text and honors child offsets', () => {\n\
           const paragraph = document.createElement('p'); const text = document.createTextNode('ab'); paragraph.appendChild(text); document.body.appendChild(paragraph);\n\
           const inside = document.createRange(); inside.setStart(text, 1); const emphasis = document.createElement('em'); emphasis.textContent = 'x'; inside.insertNode(emphasis);\n\
           assertEquals(paragraph.innerHTML, 'a<em>x</em>b');\n\
           const edge = document.createRange(); edge.setStart(paragraph, 0); const strong = document.createElement('strong'); strong.textContent = 'first'; edge.insertNode(strong);\n\
           assertEquals(paragraph.innerHTML, '<strong>first</strong>a<em>x</em>b');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range insertion test");
    assert!(
        ran.status.success(),
        "range insertion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_deletes_same_text_and_child_boundaries() {
    let dir = build_dir("t_test_dom_range_delete_simple");
    write_in(
        &dir,
        "delete-simple.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deleteContents removes text slices and complete child nodes', () => {\n\
           const text = document.createTextNode('abcdef'); document.body.appendChild(text); const characters = document.createRange();\n\
           characters.setStart(text, 2); characters.setEnd(text, 4); characters.deleteContents();\n\
           assertEquals(text.data, 'abef'); assertEquals([characters.startOffset, characters.endOffset, characters.collapsed], [2, 2, true]);\n\
           const parent = document.createElement('div'); parent.innerHTML = '<i>one</i><b>two</b><em>three</em>'; document.body.appendChild(parent); const children = document.createRange();\n\
           children.setStart(parent, 1); children.setEnd(parent, 3); children.deleteContents();\n\
           assertEquals(parent.innerHTML, '<i>one</i>'); assertEquals([children.startOffset, children.endOffset], [1, 1]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM simple range deletion test");
    assert!(
        ran.status.success(),
        "simple range deletion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_deletes_across_nested_text_boundaries() {
    let dir = build_dir("t_test_dom_range_delete_nested");
    write_in(
        &dir,
        "delete-nested.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deleteContents removes covered descendants between partial text nodes', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const range = document.createRange();\n\
           range.setStart(paragraph.firstChild, 2); range.setEnd(paragraph.lastChild, 2); range.deleteContents();\n\
           assertEquals(paragraph.innerHTML, 'onhree'); assertEquals(range.collapsed, true); assertEquals(range.toString(), '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM nested range deletion test");
    assert!(
        ran.status.success(),
        "nested range deletion test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_clones_and_extracts_partial_nested_contents() {
    let dir = build_dir("t_test_dom_range_clone_extract");
    write_in(
        &dir,
        "clone-extract.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('cloneContents preserves selected structure without mutation', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const range = document.createRange();\n\
           range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.lastChild, 2);\n\
           const copy = range.cloneContents(); const holder = document.createElement('div'); holder.appendChild(copy);\n\
           assertEquals(holder.innerHTML, 'ne <b>two</b> t'); assertEquals(paragraph.innerHTML, 'one <b>two</b> three');\n\
         });\n\
         test('extractContents returns the same structure and removes it', () => {\n\
           const paragraph = document.querySelector('p'); const range = document.createRange(); range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.lastChild, 2);\n\
           const extracted = range.extractContents(); const holder = document.createElement('div'); holder.appendChild(extracted);\n\
           assertEquals(holder.innerHTML, 'ne <b>two</b> t'); assertEquals(paragraph.innerHTML, 'ohree'); assertEquals(range.collapsed, true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range clone/extract test");
    assert!(
        ran.status.success(),
        "range clone/extract test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_range_surrounds_complete_contents_and_rejects_partial_elements() {
    let dir = build_dir("t_test_dom_range_surround");
    write_in(
        &dir,
        "surround.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('surroundContents wraps complete nodes', () => {\n\
           document.body.innerHTML = '<p>one <b>two</b> three</p>'; const paragraph = document.querySelector('p'); const bold = paragraph.querySelector('b'); const range = document.createRange();\n\
           range.selectNode(bold); const mark = document.createElement('mark'); range.surroundContents(mark);\n\
           assertEquals(paragraph.innerHTML, 'one <mark><b>two</b></mark> three'); assertEquals(range.toString(), 'two'); assertEquals(range.commonAncestorContainer, paragraph);\n\
         });\n\
         test('partial non-text nodes refuse wrapping', () => {\n\
           const paragraph = document.querySelector('p'); const range = document.createRange(); range.setStart(paragraph.firstChild, 1); range.setEnd(paragraph.querySelector('b').firstChild, 1);\n\
           assertThrows(() => range.surroundContents(document.createElement('mark')), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range surround test");
    assert!(
        ran.status.success(),
        "range surround test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_ranges_adjust_boundaries_for_tree_and_text_mutations() {
    let dir = build_dir("t_test_dom_range_adjustment");
    write_in(
        &dir,
        "adjustment.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('unrelated inserts, removals, and shortening text retain valid boundaries', () => {\n\
           const parent = document.createElement('div'); const first = document.createElement('i'); const second = document.createElement('b'); const third = document.createElement('em'); parent.append(first, second, third); document.body.appendChild(parent);\n\
           const children = document.createRange(); children.setStart(parent, 2); children.setEnd(parent, 3);\n\
           parent.insertBefore(document.createElement('strong'), second); assertEquals([children.startOffset, children.endOffset], [3, 4]);\n\
           first.remove(); assertEquals([children.startOffset, children.endOffset], [2, 3]);\n\
           const inside = document.createRange(); inside.selectNodeContents(second); second.remove(); assertEquals([inside.startContainer, inside.startOffset, inside.endContainer, inside.endOffset], [parent, 1, parent, 1]);\n\
           const text = document.createTextNode('abcdef'); parent.appendChild(text); const characters = document.createRange(); characters.setStart(text, 1); characters.setEnd(text, 5); text.data = 'xy';\n\
           assertEquals([characters.startOffset, characters.endOffset], [0, 0]);\n\
           const edited = document.createTextNode('abcdef'); parent.appendChild(edited); const span = document.createRange(); span.setStart(edited, 1); span.setEnd(edited, 5); edited.deleteData(0, 2); edited.insertData(3, 'XY');\n\
           assertEquals([span.startOffset, span.endOffset], [0, 3]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM range adjustment test");
    assert!(
        ran.status.success(),
        "range adjustment test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_document_ids_and_indexed_attributes_follow_browser_collections() {
    let dir = build_dir("t_test_dom_document_ids_and_attributes");
    write_in(
        &dir,
        "document-ids.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('document IDs and named attributes follow document order and mutations', () => {\n\
           const first = document.createElement('article'); const second = document.createElement('aside');\n\
           first.id = 'duplicate'; second.id = 'duplicate'; second.setAttribute('data-state', 'ready'); second.setAttribute('title', 'second');\n\
           document.body.append(first, second); const attributes = second.attributes;\n\
           assertEquals(document.getElementById('duplicate'), first); assertEquals(attributes[0].name, 'id'); assertEquals(attributes[1].value, 'ready'); assertEquals(attributes[3], undefined); assertEquals(attributes.item(3), null);\n\
           first.remove(); second.removeAttribute('id');\n\
           assertEquals(document.getElementById('duplicate'), null); assertEquals(attributes[0].name, 'data-state'); assertEquals(attributes.length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom document IDs and attributes");
    assert!(
        ran.status.success(),
        "DOM document ID and attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_namespace_attribute_operations_follow_named_node_maps() {
    let dir = build_dir("t_test_dom_namespace_attributes");
    write_in(
        &dir,
        "namespace-attributes.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('namespace attribute methods keep ordinary and namespaced attributes distinct', () => {\n\
           const use = document.createElement('use'); const xlink = 'http://www.w3.org/1999/xlink';\n\
           use.setAttribute('href', 'plain'); use.setAttributeNS(xlink, 'xlink:href', '#first');\n\
           assertEquals([use.getAttributeNS(null, 'href'), use.getAttributeNS(xlink, 'href'), use.hasAttributeNS(xlink, 'href')], ['plain', '#first', true]);\n\
           use.setAttributeNS(xlink, 'xlink:href', '#next'); assertEquals(use.attributes.length, 2);\n\
           const clone = use.cloneNode(); assertEquals(clone.getAttributeNS(xlink, 'href'), '#next');\n\
           use.removeAttributeNS(xlink, 'href'); assertEquals([use.hasAttribute('href'), use.hasAttributeNS(xlink, 'href')], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom namespace attributes");
    assert!(
        ran.status.success(),
        "DOM namespace attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_datasets_follow_live_data_attributes_and_html_name_conversion() {
    let dir = build_dir("t_test_dom_datasets");
    write_in(
        &dir,
        "datasets.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('dataset properties reflect data attributes, enumeration, deletion, and conversion', () => {\n\
           const element = document.createElement('article'); const dataset = element.dataset;\n\
           element.setAttribute('data-user-id', 'first'); element.setAttribute('data-ready', '');\n\
           assertEquals([dataset === element.dataset, dataset.userId, Object.keys(dataset)], [true, 'first', ['userId', 'ready']]);\n\
           dataset.userId = 42; delete dataset.ready; dataset.recordId = 'next';\n\
           assertEquals([element.getAttribute('data-user-id'), element.hasAttribute('data-ready'), element.getAttribute('data-record-id')], ['42', false, 'next']);\n\
           assertThrows(() => { dataset['record-id'] = 'no'; }, DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom datasets");
    assert!(
        ran.status.success(),
        "DOM dataset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_create_element_ns_preserves_modern_namespace_identity() {
    let dir = build_dir("t_test_dom_create_element_ns");
    write_in(
        &dir,
        "element-ns.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('createElementNS installs namespace and specialized interfaces', () => {\n\
           const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');\n\
           const use = document.createElementNS('http://www.w3.org/2000/svg', 'xlink:use');\n\
           const input = document.createElementNS('http://www.w3.org/1999/xhtml', 'input'); const div = document.createElement('div');\n\
           const math = document.createElementNS('http://www.w3.org/1998/Math/MathML', 'math');\n\
           const inputCopy = input.cloneNode(); const svgCopy = svg.cloneNode();\n\
           input.value = 'modern';\n\
           assertEquals([svg instanceof SVGElement, svg.namespaceURI, svg.nodeName, use.prefix, use.localName, input instanceof HTMLInputElement, input.value, div instanceof HTMLDivElement, math instanceof MathMLElement, inputCopy instanceof HTMLInputElement, svgCopy instanceof SVGElement], [true, 'http://www.w3.org/2000/svg', 'svg', 'xlink', 'use', true, 'modern', true, true, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom createElementNS");
    assert!(
        ran.status.success(),
        "DOM createElementNS test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_shadow_roots_keep_content_scoped_and_cross_only_composed_events() {
    let dir = build_dir("t_test_dom_shadow_root");
    write_in(
        &dir,
        "shadow.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('open and closed shadow roots preserve tree boundaries', () => {\n\
           const host = document.createElement('section'); document.body.appendChild(host);\n\
           const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button>inside</button>';\n\
           const button = root.querySelector('button'); const events = [];\n\
           host.addEventListener('go', () => events.push('host')); button.addEventListener('go', () => events.push('button'));\n\
           button.dispatchEvent(new Event('go', { bubbles: true }));\n\
           assertEquals([host.shadowRoot, root.host, root.mode, root.innerHTML, document.querySelector('button'), button.getRootNode(), button.getRootNode({ composed: true }), events], [root, host, 'open', '<button>inside</button>', null, root, document, ['button']]);\n\
           button.dispatchEvent(new Event('go', { bubbles: true, composed: true })); assertEquals(events, ['button', 'button', 'host']);\n\
           assertThrows(() => host.attachShadow({ mode: 'open' }), DOMException);\n\
           const closedHost = document.createElement('div'); const closed = closedHost.attachShadow({ mode: 'closed' });\n\
           assertEquals([closed instanceof ShadowRoot, closedHost.shadowRoot], [true, null]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM shadow-root test");
    assert!(
        ran.status.success(),
        "shadow-root test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_shadow_slots_assign_named_default_and_fallback_content() {
    let dir = build_dir("t_test_dom_shadow_slots");
    write_in(
        &dir,
        "slots.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('slots discover distributed light children and fallback content', () => {\n\
           const host = document.createElement('section');\n\
           host.innerHTML = '<h1 slot=title>Title</h1>light text<p>default</p>';\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           root.innerHTML = '<header><slot name=title><i>title fallback</i></slot></header><main><slot><em>default fallback</em></slot></main><footer><slot name=missing><b>missing fallback</b></slot></footer>';\n\
           const [title, standard, missing] = root.querySelectorAll('slot');\n\
           assertEquals([title instanceof HTMLSlotElement, title.assignedElements().map((node) => node.localName), standard.assignedNodes().map((node) => node.nodeName), missing.assignedNodes().length, missing.assignedElements({ flatten: true }).map((node) => node.localName)], [true, ['h1'], ['#text', 'P'], 0, ['b']]);\n\
           title.name = 'other';\n\
           assertEquals([title.assignedNodes().length, title.assignedElements({ flatten: true }).map((node) => node.localName)], [0, ['i']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom shadow slots");
    assert!(
        ran.status.success(),
        "DOM shadow-slot test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_composed_shadow_events_retarget_at_host_boundaries() {
    let dir = build_dir("t_test_dom_shadow_event_retargeting");
    write_in(
        &dir,
        "retarget.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('composed events expose the host outside a shadow root', () => {\n\
           const host = document.createElement('section'); document.body.appendChild(host);\n\
           const root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button>inside</button>';\n\
           const button = root.querySelector('button'); const seen = [];\n\
           button.addEventListener('go', (event) => seen.push(['button', event.target.localName]));\n\
           root.addEventListener('go', (event) => seen.push(['root', event.target.localName]));\n\
           host.addEventListener('go', (event) => seen.push(['host', event.target.localName]));\n\
           document.addEventListener('go', (event) => seen.push(['document', event.target.localName]));\n\
           button.dispatchEvent(new Event('go', { bubbles: true, composed: true }));\n\
           assertEquals(seen, [['button', 'button'], ['root', 'button'], ['host', 'section'], ['document', 'section']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom shadow event retargeting");
    assert!(
        ran.status.success(),
        "DOM shadow-event retargeting test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_deep_template_clones_preserve_inert_content() {
    let dir = build_dir("t_test_dom_template_clone");
    write_in(
        &dir,
        "template-clone.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('deep template clones copy content without populating the host', () => {\n\
           const template = document.createElement('template');\n\
           template.innerHTML = '<article data-kind=card><strong>hello</strong></article>';\n\
           const clone = template.cloneNode(true);\n\
           const imported = document.importNode(template, true);\n\
           assertEquals([template.childNodes.length, clone.childNodes.length, clone.content.childNodes.length, clone.content.firstElementChild.outerHTML, imported.content.firstElementChild.outerHTML], [0, 0, 1, '<article data-kind=\"card\"><strong>hello</strong></article>', '<article data-kind=\"card\"><strong>hello</strong></article>']);\n\
           clone.content.querySelector('strong').textContent = 'changed';\n\
           assertEquals([template.content.querySelector('strong').textContent, clone.content.querySelector('strong').textContent], ['hello', 'changed']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom template clone");
    assert!(
        ran.status.success(),
        "DOM template-clone test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_tree_walker_filters_depth_first_nodes() {
    let dir = build_dir("t_test_dom_tree_walker");
    write_in(
        &dir,
        "walker.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
      test('TreeWalker walks accepted nodes and rejects subtrees', () => {\n\
        const root = document.createElement('div'); root.innerHTML = '<i>one</i><!--note--><b><em>two</em></b>';\n\
        const all = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_COMMENT);\n\
        const names = []; for (let node; node = all.nextNode();) names.push(node.nodeName);\n\
        const filtered = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT, node => node.localName === 'b' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT);\n\
        const kept = []; for (let node; node = filtered.nextNode();) kept.push(node.nodeName);\n\
        assertEquals([names, kept], [['I', '#comment', 'B', 'EM'], ['I']]);\n\
      });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM tree-walker test");
    assert!(
        ran.status.success(),
        "tree-walker test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_class_lists_track_attributes_and_validate_tokens() {
    let dir = build_dir("t_test_dom_class_lists");
    write_in(
        &dir,
        "class-list.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('class list mutations preserve ordered unique tokens', () => {\n\
           const element = document.createElement('div'); element.className = 'one one two'; const classes = element.classList;\n\
           assertEquals(classes, element.classList); assertEquals([...classes], ['one', 'two']); assertEquals(classes[1], 'two'); assertEquals(classes.item(2), null);\n\
           classes.add('three', 'one'); classes.remove('two'); assertEquals(element.className, 'one three');\n\
           assertEquals(classes.replace('one', 'first'), true); assertEquals(classes.toggle('three'), false); assertEquals(classes.toggle('four', true), true);\n\
           assertEquals(element.getAttribute('class'), 'first four');\n\
         });\n\
         test('class list rejects invalid tokens without partial changes', () => {\n\
           const element = document.createElement('div'); element.className = 'ready';\n\
           assertThrows(() => element.classList.add('next', 'bad token'), DOMException); assertEquals(element.classList.contains(''), false);\n\
           assertEquals(element.className, 'ready');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom class lists");
    assert!(
        ran.status.success(),
        "DOM class list test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_does_not_call_overridden_element_append_child() {
    let dir = build_dir("t_test_dom_parser_public_append");
    write_in(
        &dir,
        "parser-append.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('parser construction bypasses public appendChild overrides', () => {
             const appendChild = Element.prototype.appendChild;
             Element.prototype.appendChild = function() { throw new Error('public append'); };
             try {
               const template = document.createElement('template');
               template.innerHTML = '<ul><li>one</li><li>two</li></ul>';
               assertEquals(template.content.querySelectorAll('li').length, 2);
             } finally {
               Element.prototype.appendChild = appendChild;
             }
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn parser public append test");
    assert!(
        ran.status.success(),
        "parser public append test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_keeps_svg_foreign_object_namespaces() {
    let dir = build_dir("t_test_dom_svg_foreign_object_namespaces");
    write_in(
        &dir,
        "foreign-object.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('parsed SVG foreignObject enters and exits the HTML namespace', () => {\n\
           const createElementNS = document.createElementNS; let htmlNamespaceCalls = 0; document.createElementNS = function(namespace, name) { if (namespace === 'http://www.w3.org/1999/xhtml') htmlNamespaceCalls += 1; return createElementNS.call(this, namespace, name); };\n\
           document.body.innerHTML = '<svg><foreignObject><div id=html-child>ok</div></foreignObject><clipPath id=\"clip\"/></svg>';\n\
           document.createElementNS = createElementNS; assertEquals(htmlNamespaceCalls, 0);\n\
           const foreignObject = document.body.querySelector('foreignObject'); const htmlChild = foreignObject.querySelector('#html-child');\n\
           assertEquals([foreignObject.tagName, foreignObject.namespaceURI, htmlChild.namespaceURI], ['foreignObject', 'http://www.w3.org/2000/svg', 'http://www.w3.org/1999/xhtml']);\n\
           assertEquals(document.body.querySelector('clipPath').tagName, 'clipPath');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom SVG foreignObject namespaces");
    assert!(
        ran.status.success(),
        "DOM SVG foreignObject namespace test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_selection_keeps_logical_range_boundaries() {
    let dir = build_dir("t_test_dom_selection");
    write_in(
        &dir,
        "selection.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('window and document expose the realm selection', () => {
             const text = document.createTextNode('hello'); document.body.appendChild(text);
             const range = document.createRange(); range.setStart(text, 2); range.collapse(true);
             const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
             assertEquals([selection === document.getSelection(), selection.rangeCount, selection.isCollapsed, selection.getRangeAt(0).startOffset], [true, 1, true, 2]);
             selection.removeAllRanges(); assertEquals(selection.rangeCount, 0);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM selection test");
    assert!(
        ran.status.success(),
        "DOM selection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_elements_upgrade_and_react_to_tree_changes() {
    let dir = build_dir("t_test_dom_custom_elements");
    write_in(
        &dir,
        "custom-elements.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('definition upgrades existing elements and runs reactions', async () => {\n\
           const calls = [];\n\
           const prior = document.createElement('x-probe'); prior.setAttribute('state', 'ready'); document.body.appendChild(prior);\n\
           const pending = customElements.whenDefined('x-probe');\n\
           class Probe extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             constructor() { super(); calls.push('constructed'); }\n\
             connectedCallback() { calls.push('connected'); }\n\
             disconnectedCallback() { calls.push('disconnected'); }\n\
             adoptedCallback() { calls.push('adopted'); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-probe', Probe);\n\
           assertEquals(await pending, Probe);\n\
           assertEquals(prior instanceof Probe, true);\n\
           assertEquals(calls, ['constructed', 'state:null:ready', 'connected']);\n\
           prior.setAttribute('state', 'next'); prior.remove();\n\
           const secondDocument = new Document(); secondDocument.adoptNode(prior);\n\
           assertEquals(calls, ['constructed', 'state:null:ready', 'connected', 'state:ready:next', 'disconnected', 'adopted']);\n\
         });\n\
         test('the registry validates names and duplicate definitions', () => {\n\
           class Good extends HTMLElement {}\n\
           customElements.define('x-good', Good);\n\
           assertEquals(document.createElement('x-good') instanceof Good, true);\n\
           assertThrows(() => customElements.define('plain', Good), DOMException);\n\
           assertThrows(() => customElements.define('x-good', Good), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom custom elements");
    assert!(
        ran.status.success(),
        "DOM custom-element test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_connected_callbacks_do_not_repeat_for_new_descendants() {
    let dir = build_dir("t_test_dom_connected_callback_descendant");
    write_in(
        &dir,
        "connected.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('a descendant inserted by a connected callback reacts once', () => {
             let childConnections = 0;
             class Child extends HTMLElement { connectedCallback() { childConnections += 1; } }
             class Parent extends HTMLElement { connectedCallback() { this.appendChild(document.createElement('x-connected-child')); } }
             customElements.define('x-connected-child', Child); customElements.define('x-connected-parent', Parent);
             document.body.appendChild(document.createElement('x-connected-parent'));
             assertEquals(childConnections, 1);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn connected custom-element descendant test");
    assert!(
        ran.status.success(),
        "connected custom-element descendant test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_element_registry_validates_and_settles_waiters() {
    let dir = build_dir("t_test_dom_custom_registry");
    write_in(
        &dir,
        "registry.test.mjs",
        "import { test, assertEquals, assertThrows, assertRejects } from 'runtime:test';\n\
         test('registry contracts', async () => {\n\
           assertEquals(customElements.get('x-later'), undefined);\n\
           assertRejects(() => customElements.whenDefined('plain'), DOMException);\n\
           assertThrows(() => customElements.define('plain', class Plain extends HTMLElement {}), DOMException);\n\
           assertThrows(() => customElements.define('x-invalid', () => {}), TypeError);\n\
           const waiting = customElements.whenDefined('x-later');\n\
           class Later extends HTMLElement {}\n\
           customElements.define('x-later', Later);\n\
           assertEquals(await waiting, Later);\n\
           assertEquals(customElements.get('x-later'), Later);\n\
           assertThrows(() => customElements.define('x-later', Later), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom registry test");
    assert!(
        ran.status.success(),
        "registry test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_elements_upgrade_detached_and_parser_nodes() {
    let dir = build_dir("t_test_dom_custom_upgrade");
    write_in(
        &dir,
        "upgrade.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('detached and parsed upgrades', () => {\n\
           const calls = [];\n\
           const detached = document.createElement('x-detached');\n\
           class Detached extends HTMLElement { constructor() { super(); calls.push('detached constructed'); } connectedCallback() { calls.push('detached connected'); } }\n\
           customElements.define('x-detached', Detached);\n\
           assertEquals(detached instanceof Detached, false);\n\
           customElements.upgrade(detached);\n\
           assertEquals(detached instanceof Detached, true);\n\
           document.body.appendChild(detached);\n\
           class Parsed extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             constructor() { super(); calls.push('parsed constructed'); }\n\
             connectedCallback() { calls.push('parsed connected'); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-parsed', Parsed);\n\
           document.body.innerHTML = '<x-parsed state=ready></x-parsed>';\n\
           assertEquals(document.body.firstChild instanceof Parsed, true);\n\
           assertEquals(calls, ['detached constructed', 'detached connected', 'parsed constructed', 'state:null:ready', 'parsed connected']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom upgrade test");
    assert!(
        ran.status.success(),
        "upgrade test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_custom_element_reactions_filter_attributes_and_track_moves() {
    let dir = build_dir("t_test_dom_custom_reactions");
    write_in(
        &dir,
        "reactions.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('attribute filtering and tree reaction order', () => {\n\
           const calls = [];\n\
           class Life extends HTMLElement {\n\
             static observedAttributes = ['state'];\n\
             connectedCallback() { calls.push(`connected:${this.id}`); }\n\
             disconnectedCallback() { calls.push(`disconnected:${this.id}`); }\n\
             attributeChangedCallback(name, oldValue, newValue) { calls.push(`${name}:${oldValue}:${newValue}`); }\n\
           }\n\
           customElements.define('x-life', Life);\n\
           const one = document.createElement('x-life'); one.id = 'one';\n\
           const two = document.createElement('x-life'); two.id = 'two';\n\
           const fragment = document.createDocumentFragment(); fragment.append(one, two);\n\
           document.body.appendChild(fragment);\n\
           one.setAttribute('other', 'ignored'); one.setAttribute('state', 'one'); one.setAttribute('state', 'one'); one.removeAttribute('state');\n\
           document.body.appendChild(one); two.remove();\n\
           assertEquals(calls, ['connected:one', 'connected:two', 'state:null:one', 'state:one:one', 'state:one:null', 'disconnected:one', 'connected:one', 'disconnected:two']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn custom reaction test");
    assert!(
        ran.status.success(),
        "reaction test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_delivers_child_list_records_at_microtasks() {
    let dir = build_dir("t_test_dom_mutation_child_list");
    write_in(
        &dir,
        "child-list.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('child-list records batch until the microtask checkpoint', async () => {\n\
           const parent = document.createElement('section'); document.body.appendChild(parent);\n\
           const first = document.createElement('i'); const second = document.createElement('b');\n\
           const deliveries = []; const observer = new MutationObserver((records, same) => deliveries.push([same === observer, records]));\n\
           observer.observe(parent, { childList: true });\n\
           parent.append(first, second); parent.removeChild(first);\n\
           assertEquals(deliveries.length, 0); await Promise.resolve();\n\
           assertEquals(deliveries.length, 1);\n\
           const records = deliveries[0][1]; assertEquals(deliveries[0][0], true); assertEquals(records.length, 2);\n\
           assertEquals(records.map((record) => [record.type, record.target, [...record.addedNodes], [...record.removedNodes], record.previousSibling, record.nextSibling]), [\n\
             ['childList', parent, [first, second], [], null, null],\n\
             ['childList', parent, [], [first], null, second],\n\
           ]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation child-list test");
    assert!(
        ran.status.success(),
        "mutation child-list test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_filters_attributes_and_preserves_old_values() {
    let dir = build_dir("t_test_dom_mutation_attributes");
    write_in(
        &dir,
        "attributes.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('attribute filters and old values', async () => {\n\
           const element = document.createElement('div'); document.body.appendChild(element);\n\
           const observer = new MutationObserver(() => {});\n\
           observer.observe(element, { attributes: true, attributeFilter: ['state'], attributeOldValue: true });\n\
           element.setAttribute('other', 'ignored'); element.setAttribute('state', 'first'); element.setAttribute('state', 'second'); element.removeAttribute('state');\n\
           const records = observer.takeRecords();\n\
           assertEquals(records.map((record) => [record.attributeName, record.oldValue, record.attributeNamespace]), [['state', null, null], ['state', 'first', null], ['state', 'second', null]]);\n\
           assertThrows(() => observer.observe(element, {}), TypeError);\n\
           assertThrows(() => observer.observe(element, { attributes: false, attributeOldValue: true }), TypeError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation attribute test");
    assert!(
        ran.status.success(),
        "mutation attribute test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_tracks_subtree_character_data_and_disconnects() {
    let dir = build_dir("t_test_dom_mutation_character_data");
    write_in(
        &dir,
        "character-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('subtree character data and observer lifecycle', async () => {\n\
           const parent = document.createElement('div'); const child = document.createElement('span'); const text = document.createTextNode('before');\n\
           child.appendChild(text); parent.appendChild(child); document.body.appendChild(parent);\n\
           const calls = []; const observer = new MutationObserver((records) => calls.push(records));\n\
           observer.observe(parent, { characterData: true, characterDataOldValue: true, subtree: true });\n\
           text.data = 'after'; await Promise.resolve();\n\
           assertEquals(calls.length, 1); assertEquals(calls[0].map((record) => [record.type, record.target, record.oldValue]), [['characterData', text, 'before']]);\n\
           observer.disconnect(); text.data = 'final'; await Promise.resolve();\n\
           assertEquals(calls.length, 1); assertEquals(observer.takeRecords(), []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation character-data test");
    assert!(
        ran.status.success(),
        "mutation character-data test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_mutation_observer_reschedules_changes_made_by_its_callback() {
    let dir = build_dir("t_test_dom_mutation_reschedule");
    write_in(
        &dir,
        "reschedule.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('callback changes are delivered in a later microtask', async () => {\n\
           const element = document.createElement('div'); document.body.appendChild(element); const batches = [];\n\
           const observer = new MutationObserver((records) => { batches.push(records.map((record) => record.attributeName)); if (batches.length === 1) element.setAttribute('second', 'yes'); });\n\
           observer.observe(element, { attributes: true }); element.setAttribute('first', 'yes');\n\
           await Promise.resolve(); assertEquals(batches, [['first']]);\n\
           await Promise.resolve(); assertEquals(batches, [['first'], ['second']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM mutation reschedule test");
    assert!(
        ran.status.success(),
        "mutation reschedule test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
