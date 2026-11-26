//! `esdev test`'s DOM: the window, events, focus, storage, the clock and the declarations.

use super::*;

#[test]
fn test_dom_declarations_know_which_properties_exist_and_cookies_round_trip() {
    let dir = build_dir("t_test_dom_properties_cookies");
    write_in(
        &dir,
        "properties.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('an unknown property reads undefined, a known one reads empty', () => {\n\
           const element = document.createElement('div');\n\
           document.body.append(element);\n\
           element.style.color = 'red';\n\
           const computed = getComputedStyle(element);\n\
           assertEquals([element.style.color, element.style.transform, element.style.nonsenseProp], ['red', '', undefined]);\n\
           // `transform`'s initial value is a keyword, so it is answered.\n\
           assertEquals([computed.transform, computed.nonsenseProp], ['none', undefined]);\n\
           assertEquals(['transform' in computed, 'nonsenseProp' in computed, 'gridTemplateAreas' in computed], [true, false, true]);\n\
           element.style.setProperty('--x', '1px');\n\
           assertEquals(getComputedStyle(element).getPropertyValue('--x'), '1px');\n\
         });\n\
         test('supports answers from the known properties', () => {\n\
           assertEquals([CSS.supports('color', 'red'), CSS.supports('display', 'grid')], [true, true]);\n\
           assertEquals([CSS.supports('nonsense-prop', '1'), CSS.supports('(nonsense-prop: 1)')], [false, false]);\n\
           assertEquals(CSS.supports('not (nonsense-prop: 1)'), true);\n\
           const style = document.createElement('style');\n\
           style.textContent = '@supports (display: grid) { .s { font-style: italic } } @supports (nonsense: 1) { .s { font-style: oblique } }';\n\
           document.head.append(style);\n\
           const element = document.createElement('p');\n\
           element.className = 's';\n\
           document.body.append(element);\n\
           assertEquals(getComputedStyle(element).fontStyle, 'italic');\n\
           style.remove();\n\
         });\n\
         test('cookies are a document-level string store', () => {\n\
           assertEquals([document.cookie, 'cookie' in document], ['', true]);\n\
           document.cookie = 'a=1';\n\
           document.cookie = 'b=2; Path=/; Secure';\n\
           assertEquals(document.cookie, 'a=1; b=2');\n\
           document.cookie = 'a=updated';\n\
           assertEquals(document.cookie, 'a=updated; b=2');\n\
           document.cookie = 'a=; Max-Age=0';\n\
           assertEquals(document.cookie, 'b=2');\n\
           document.cookie = 'c=3; Expires=Thu, 01 Jan 1970 00:00:00 GMT';\n\
           assertEquals(document.cookie, 'b=2');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom properties and cookies");
    assert!(
        ran.status.success(),
        "DOM property and cookie test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_events_propagate_through_the_window() {
    let dir = build_dir("t_test_dom_window_propagation");
    write_in(
        &dir,
        "window-path.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('the window is the last target in the path', () => {\n\
           const target = document.createElement('i');\n\
           document.body.append(target);\n\
           const order = [];\n\
           window.addEventListener('z', () => order.push('window-capture'), true);\n\
           document.addEventListener('z', () => order.push('document-capture'), true);\n\
           target.addEventListener('z', event => order.push(event.currentTarget === target ? 'target' : 'wrong'));\n\
           window.addEventListener('z', event => order.push(event.currentTarget === window && event.target === target ? 'window-bubble' : 'wrong'));\n\
           const event = new Event('z', { bubbles: true });\n\
           target.dispatchEvent(event);\n\
           assertEquals(order, ['window-capture', 'document-capture', 'target', 'window-bubble']);\n\
         });\n\
         test('composedPath crosses the shadow boundary and ends at the window', () => {\n\
           const host = document.createElement('div');\n\
           const root = host.attachShadow({ mode: 'open' });\n\
           const inner = document.createElement('span');\n\
           root.append(inner);\n\
           document.body.append(host);\n\
           let path = [];\n\
           host.addEventListener('x', event => { path = event.composedPath(); });\n\
           inner.dispatchEvent(new Event('x', { bubbles: true, composed: true }));\n\
           assertEquals([path.length, path.at(-1) === window, path.at(-2) === document], [7, true, true]);\n\
         });\n\
         test('a load event at the document stops there', () => {\n\
           const seen = [];\n\
           window.addEventListener('load', () => seen.push('window'));\n\
           document.addEventListener('load', () => seen.push('document'));\n\
           document.dispatchEvent(new Event('load', { bubbles: true }));\n\
           assertEquals(seen, ['document']);\n\
         });\n\
         test('an unqualified listener call reaches the window', () => {\n\
           let hits = 0;\n\
           globalThis.__probe = () => hits++;\n\
           (0, eval)(\"addEventListener('bare', globalThis.__probe)\");\n\
           window.dispatchEvent(new Event('bare'));\n\
           assertEquals(hits, 1);\n\
         });\n\
         test('the window dispatches its own events', () => {\n\
           let seen = null;\n\
           window.addEventListener('own', event => { seen = [event.currentTarget === window, event.target === window, event.composedPath().length]; });\n\
           assertEquals(window.dispatchEvent(new Event('own')), true);\n\
           assertEquals(seen, [true, true, 1]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom window propagation");
    assert!(
        ran.status.success(),
        "DOM window propagation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_abort_signals_dispatch_in_their_own_event_realm() {
    let dir = build_dir("t_test_dom_abort_signal_events");
    write_in(
        &dir,
        "abort.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('abort signals notify DOM event targets', () => {\n\
           const controller = new AbortController();\n\
           const target = document.createElement('button');\n\
           let abort = null; let calls = 0;\n\
           controller.signal.addEventListener('abort', event => { abort = [event.type, event.isTrusted]; });\n\
           target.addEventListener('go', () => calls++, { signal: controller.signal });\n\
           controller.abort('finished');\n\
           target.dispatchEvent(new Event('go'));\n\
           assertEquals([controller.signal.aborted, controller.signal.reason, abort, calls], [true, 'finished', ['abort', true], 0]);\n\
           const root = new AbortController();\n\
           const signals = [root.signal, AbortSignal.any([root.signal]), AbortSignal.any([root.signal])];\n\
           signals.push(AbortSignal.any([signals[0]]), AbortSignal.any([signals[1]]));\n\
           const order = []; signals.forEach((signal, index) => signal.addEventListener('abort', () => order.push(index)));\n\
           root.abort();\n\
           assertEquals(order, [0, 1, 2, 3, 4]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom abort signal events");
    assert!(
        ran.status.success(),
        "DOM abort-signal test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_event_targets_honor_listener_options_and_event_state() {
    let dir = build_dir("t_test_dom_event_target_options");
    write_in(
        &dir,
        "events.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('event target listener options and state', () => {\n\
           const target = new EventTarget();\n\
           const passive = new Event('passive', { cancelable: true });\n\
           target.addEventListener('passive', event => { event.returnValue = false; }, { passive: true });\n\
           assertEquals(target.dispatchEvent(passive), true); assertEquals(passive.defaultPrevented, false);\n\
           assertThrows(() => target.addEventListener('x', null, { signal: null }), TypeError);\n\
           let duringDispatch; target.addEventListener('state', { get handleEvent() { duringDispatch = window.event; return () => {}; } });\n\
           const state = new Event('state'); target.dispatchEvent(state);\n\
           assertEquals([duringDispatch, state.srcElement, state.composedPath().length, typeof Object.getOwnPropertyDescriptor(state, 'isTrusted').get], [state, target, 0, 'function']);\n\
           assertEquals(state.timeStamp > 0, true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom event target options");
    assert!(
        ran.status.success(),
        "DOM event-target-options test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `navigator` is an ordinary object, as a browser's is: a library can add to
/// it, and user-event's `setup()` installs its clipboard stub that way.
#[test]
fn test_dom_navigator_takes_properties_of_its_own() {
    run_dom_test(
        "t_test_dom_navigator",
        "navigator.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('navigator is a Navigator that takes properties', () => {
  assertEquals(navigator instanceof Navigator, true);
  assertEquals(Object.prototype.toString.call(navigator), '[object Navigator]');
  assertEquals([navigator.userAgent, navigator.language, navigator.languages[0]], ['esdev DOM', 'en-US', 'en-US']);
  const clipboard = {};
  Object.defineProperty(navigator, 'clipboard', { get: () => clipboard, configurable: true });
  assertEquals(navigator.clipboard, clipboard);
  delete navigator.clipboard;
  assertEquals(navigator.clipboard, undefined);
});
"#,
        "DOM navigator test did not run",
    );
}

/// `setBaseAndExtent()` and `extend()` keep the anchor where it was put, so a
/// selection made right to left reports its anchor at the range's end.
#[test]
fn test_dom_selection_keeps_its_anchor_and_focus() {
    run_dom_test(
        "t_test_dom_selection_extent",
        "extent.test.mjs",
        r#"import { test, assertEquals, assertThrows } from 'runtime:test';
test('setBaseAndExtent and extend', () => {
  document.body.innerHTML = '<p>hello world</p>';
  const text = document.querySelector('p').firstChild;
  const selection = document.getSelection();
  selection.setBaseAndExtent(text, 1, text, 4);
  assertEquals([selection.anchorOffset, selection.focusOffset, String(selection)], [1, 4, 'ell']);
  assertEquals([selection.getRangeAt(0).startOffset, selection.getRangeAt(0).endOffset], [1, 4]);
  selection.setBaseAndExtent(text, 5, text, 2);
  assertEquals([selection.anchorOffset, selection.focusOffset, String(selection)], [5, 2, 'llo']);
  assertEquals([selection.getRangeAt(0).startOffset, selection.getRangeAt(0).endOffset], [2, 5]);
  selection.extend(text, 8);
  assertEquals([selection.anchorOffset, selection.focusOffset, selection.isCollapsed], [5, 8, false]);
  assertThrows(() => selection.setBaseAndExtent(text, 99, text, 0), DOMException);
  selection.removeAllRanges();
  assertThrows(() => selection.extend(text, 1), DOMException);
});
"#,
        "DOM selection extent test did not run",
    );
}

/// `FileReader` reads a Blob through events, as user-event's clipboard does.
#[test]
fn test_dom_file_reader_reads_blobs_through_events() {
    run_dom_test(
        "t_test_dom_file_reader",
        "reader.test.mjs",
        r#"import { test, assertEquals, assertThrows } from 'runtime:test';
const read = (method, blob, ...args) => new Promise((resolve, reject) => {
  const reader = new FileReader();
  const seen = [];
  for (const type of ['loadstart', 'progress', 'load', 'loadend']) reader.addEventListener(type, () => seen.push(type));
  reader.onerror = reject;
  reader.onloadend = () => resolve({ result: reader.result, seen, state: reader.readyState });
  reader[method](blob, ...args);
  assertEquals(reader.readyState, FileReader.LOADING);
});
test('every read shape', async () => {
  const blob = new Blob(['héllo'], { type: 'text/plain' });
  const text = await read('readAsText', blob);
  assertEquals([text.result, text.state], ['héllo', FileReader.DONE]);
  assertEquals(text.seen, ['loadstart', 'progress', 'load', 'loadend']);
  assertEquals(new Uint8Array((await read('readAsArrayBuffer', blob)).result).length, 6);
  assertEquals((await read('readAsDataURL', new Blob(['hi'], { type: 'text/plain' }))).result, 'data:text/plain;base64,aGk=');
  assertEquals((await read('readAsBinaryString', new Blob([new Uint8Array([104, 255])]))).result, 'hÿ');
});
test('abort and refusals', async () => {
  const reader = new FileReader();
  const seen = [];
  reader.onabort = () => seen.push('abort');
  reader.onload = () => seen.push('load');
  reader.readAsText(new Blob(['x']));
  assertThrows(() => reader.readAsText(new Blob(['y'])), DOMException);
  reader.abort();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assertEquals([seen, reader.result, reader.readyState], [['abort'], null, FileReader.DONE]);
  assertThrows(() => new FileReader().readAsText('not a blob'), TypeError);
});
"#,
        "DOM FileReader test did not run",
    );
}

#[test]
fn test_dom_inline_handlers_run_for_modern_events() {
    let dir = build_dir("t_test_dom_inline_events");
    write_in(
        &dir,
        "inline-events.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('modern Event dispatch invokes inline handlers', () => {\n\
           const button = document.createElement('button'); const calls = [];\n\
           button.onclick = (event) => { calls.push(event.type); event.preventDefault(); };\n\
           const event = new Event('click', { bubbles: true, cancelable: true });\n\
           assertEquals([button.dispatchEvent(event), calls], [false, ['click']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom inline events");
    assert!(
        ran.status.success(),
        "DOM inline event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_html_element_reflects_editing_and_interaction_state() {
    let dir = build_dir("t_test_dom_html_interaction_reflection");
    write_in(
        &dir,
        "interaction.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('common HTMLElement state matches browser defaults and reflection', () => {
             const div = document.createElement('div');
             const anchor = document.createElement('a'); anchor.href = '#target';
             const input = document.createElement('input');
             assertEquals([div.contentEditable, div.isContentEditable, div.translate, div.draggable, div.spellcheck, div.tabIndex, anchor.draggable, anchor.tabIndex, input.tabIndex], ['inherit', false, true, false, true, -1, true, 0, 0]);
             div.contentEditable = 'plaintext-only'; div.translate = false; div.draggable = true; div.spellcheck = false; div.tabIndex = 3;
             assertEquals([div.isContentEditable, div.getAttribute('contenteditable'), div.getAttribute('translate'), div.getAttribute('draggable'), div.getAttribute('spellcheck'), div.getAttribute('tabindex')], [true, 'plaintext-only', 'no', 'true', 'false', '3']);
             const child = document.createElement('span'); div.appendChild(child);
             assertEquals(child.isContentEditable, true);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn HTMLElement interaction reflection test");
    assert!(
        ran.status.success(),
        "HTMLElement interaction reflection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_canvas_elements_reflect_bitmap_dimensions() {
    let dir = build_dir("t_test_dom_canvas_dimensions");
    write_in(
        &dir,
        "canvas.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('canvas dimensions are reflected integer properties', () => {
             const canvas = document.createElement('canvas');
             assertEquals(canvas instanceof HTMLCanvasElement, true);
             assertEquals([canvas.width, canvas.height], [300, 150]);
             canvas.width = 80; canvas.height = 40;
             assertEquals([canvas.getAttribute('width'), canvas.getAttribute('height'), canvas.width, canvas.height], ['80', '40', 80, 40]);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn canvas dimension reflection test");
    assert!(
        ran.status.success(),
        "canvas dimension reflection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_tracks_active_element_and_event_order() {
    let dir = build_dir("t_test_dom_focus_events");
    write_in(
        &dir,
        "focus-events.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('focus transfer has browser event ordering', () => {\n\
           const first = document.createElement('input'); const second = document.createElement('button');\n\
           document.body.append(first, second);\n\
           const calls = [];\n\
           for (const [element, name] of [[first, 'first'], [second, 'second']]) {\n\
             for (const type of ['blur', 'focusout', 'focus', 'focusin']) element.addEventListener(type, (event) => calls.push(`${name}:${type}:${event.relatedTarget === first ? 'first' : event.relatedTarget === second ? 'second' : 'body'}`));\n\
           }\n\
           assertEquals(document.activeElement, document.body);\n\
           first.focus(); assertEquals(document.activeElement, first);\n\
           second.focus(); assertEquals(document.activeElement, second);\n\
           assertEquals(calls, ['first:focus:body', 'first:focusin:body', 'first:blur:second', 'first:focusout:second', 'second:focus:first', 'second:focusin:first']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus event test");
    assert!(
        ran.status.success(),
        "focus event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focusability_is_connection_and_attribute_driven() {
    let dir = build_dir("t_test_dom_focusability");
    write_in(
        &dir,
        "focusability.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('only connected enabled native or tabindex elements focus', () => {\n\
           const plain = document.createElement('div'); const indexed = document.createElement('div'); indexed.tabIndex = -1;\n\
           const disabled = document.createElement('button'); disabled.disabled = true;\n\
           const hidden = document.createElement('input'); hidden.type = 'hidden';\n\
           const link = document.createElement('a'); const detached = document.createElement('input');\n\
           document.body.append(plain, indexed, disabled, hidden, link);\n\
           plain.focus(); assertEquals(document.activeElement, document.body);\n\
           disabled.focus(); hidden.focus(); detached.focus(); assertEquals(document.activeElement, document.body);\n\
           indexed.focus(); assertEquals(document.activeElement, indexed);\n\
           link.focus(); assertEquals(document.activeElement, indexed);\n\
           link.setAttribute('href', '/target'); link.focus(); assertEquals(document.activeElement, link);\n\
           link.blur(); assertEquals(document.activeElement, document.body);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focusability test");
    assert!(
        ran.status.success(),
        "focusability test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_focus_returns_to_body_when_the_active_subtree_is_removed() {
    let dir = build_dir("t_test_dom_focus_removal");
    write_in(
        &dir,
        "focus-removal.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('removing a focused descendant clears focus silently', () => {\n\
           const wrapper = document.createElement('section'); const input = document.createElement('input');\n\
           let events = [];\n\
           for (const type of ['blur', 'focusout', 'focus', 'focusin']) {\n\
             input.addEventListener(type, () => events.push(type));\n\
             document.body.addEventListener(type, () => events.push(`body:${type}`), true);\n\
           }\n\
           wrapper.appendChild(input); document.body.appendChild(wrapper); input.focus();\n\
           events = [];\n\
           wrapper.remove();\n\
           // A removal resets the focus and fires nothing: the element that would\n\
           // receive the event is no longer in the document. Verified upstream by\n\
           // dom/nodes/insertion-removing-steps/blur-event.window.js.\n\
           assertEquals(document.activeElement, document.body); assertEquals(events, []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM focus removal test");
    assert!(
        ran.status.success(),
        "focus removal test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_console_methods_are_writable() {
    let dir = build_dir("t_test_dom_browser_console");
    write_in(
        &dir,
        "console.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
           test('console methods are writable own properties', () => {
             const descriptor = Object.getOwnPropertyDescriptor(console, 'warn');
             const previous = console.warn; let called = false;
             console.warn = () => { called = true; };
             console.warn('test');
             console.warn = previous;
             assertEquals([descriptor.writable, descriptor.configurable, called], [true, true, true]);
           });
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn browser console DOM test");
    assert!(
        ran.status.success(),
        "browser console DOM test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_location_and_history_are_in_memory() {
    let dir = build_dir("t_test_dom_history");
    write_in(
        &dir,
        "history.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('location and history', () => {\n\
           const popped = []; window.addEventListener('popstate', (event) => popped.push(event.state.page));\n\
           history.pushState({ page: 1 }, '', '/one?x=1#top');\n\
           history.pushState({ page: 2 }, '', '/two');\n\
           assertEquals(location.pathname, '/two'); assertEquals(history.length, 3); assertEquals(history.state.page, 2);\n\
           history.back(); assertEquals(location.href, 'http://localhost/one?x=1#top'); assertEquals(history.state.page, 1);\n\
           history.forward(); assertEquals(popped, [1, 2]);\n\
           history.replaceState({ page: 3 }, '', '/three');\n\
           assertEquals(location.pathname, '/three'); assertEquals(history.state.page, 3);\n\
           location.assign('/recorded'); assertEquals(location.pathname, '/recorded');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM history test");
    assert!(
        ran.status.success(),
        "history test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_storage_and_navigator_are_realm_local() {
    let dir = build_dir("t_test_dom_storage");
    write_in(
        &dir,
        "storage.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('storage and navigator', () => {\n\
           localStorage.setItem('answer', 42); sessionStorage.setItem('answer', 'session');\n\
           assertEquals(localStorage.getItem('answer'), '42'); assertEquals(sessionStorage.getItem('answer'), 'session');\n\
           assertEquals(localStorage.key(0), 'answer'); assertEquals(localStorage.key(1), null);\n\
           localStorage.removeItem('answer'); sessionStorage.clear();\n\
           assertEquals(localStorage.length, 0); assertEquals(sessionStorage.length, 0);\n\
           assertEquals(navigator.userAgent, 'esdev DOM'); assertEquals(navigator.language, 'en-US'); assertEquals(navigator.languages, ['en-US']);\n\
           assertEquals(window, globalThis);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM storage test");
    assert!(
        ran.status.success(),
        "storage test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_storage_is_not_shared_by_isolated_test_files() {
    let dir = build_dir("t_test_dom_storage_isolation");
    write_in(
        &dir,
        "first.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('writes storage', () => { localStorage.setItem('leak', 'no'); });\n",
    );
    write_in(
        &dir,
        "second.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('starts with clean storage', () => { assertEquals(localStorage.getItem('leak'), null); });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom", "--jobs=1"])
        .output()
        .expect("spawn DOM storage isolation test");
    assert!(
        ran.status.success(),
        "storage isolation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_window_media_queries_read_the_viewport_and_observers_never_deliver() {
    let dir = build_dir("t_test_dom_window_stubs");
    write_in(
        &dir,
        "stubs.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('computed styles and stubs', () => {\n\
           const element = document.createElement('div'); element.style.width = '10px';\n\
           // Outside the tree there is no computed style at all, as in a browser.\n\
           assertEquals(getComputedStyle(element).length, 0);\n\
           document.body.append(element);\n\
           const computed = getComputedStyle(element);\n\
           assertEquals(computed.width, '10px'); assertEquals(computed.getPropertyValue('width'), '10px');\n\
           assertThrows(() => { computed.width = '20px'; }, TypeError);\n\
           const query = matchMedia('(min-width: 1px)');\n\
           assertEquals([query.matches, query.media], [true, '(min-width: 1px)']);\n\
           assertEquals([window.innerWidth, window.innerHeight], [1024, 768]);\n\
           assertEquals(matchMedia('(min-width: 99999px)').matches, false);\n\
           window.innerWidth = 320;\n\
           assertEquals([query.matches, matchMedia('(min-width: 400px)').matches], [true, false]);\n\
           window.innerWidth = 1024;\n\
           let delivered = 0; const resize = new ResizeObserver(() => { delivered += 1; }); resize.observe(element);\n\
           const intersection = new IntersectionObserver(() => { delivered += 1; }); intersection.observe(element);\n\
           assertEquals(resize.takeRecords(), []); assertEquals(intersection.takeRecords(), []); assertEquals(delivered, 0);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM window stub test");
    assert!(
        ran.status.success(),
        "window stub test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_animation_frames_follow_the_test_clock() {
    let dir = build_dir("t_test_dom_raf");
    write_in(
        &dir,
        "raf.test.mjs",
        "import { test, assertEquals, clock } from 'runtime:test';\n\
         test('animation frames', () => {\n\
           clock.freeze(new Date('2020-01-01T00:00:00Z'));\n\
           const frames = []; const cancelled = requestAnimationFrame(() => frames.push('cancelled'));\n\
           cancelAnimationFrame(cancelled); requestAnimationFrame((at) => frames.push(at));\n\
           clock.advance(15); assertEquals(frames, []);\n\
           clock.advance(1); assertEquals(frames, [performance.now()]);\n\
           clock.release();\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM rAF test");
    assert!(
        ran.status.success(),
        "rAF test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
