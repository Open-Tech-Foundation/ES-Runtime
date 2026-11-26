//! `esdev test`'s DOM: forms, their controls, validation, submission and reset.

use super::*;

#[test]
fn test_dom_element_internals_associate_custom_elements_with_forms() {
    let dir = build_dir("t_test_dom_element_internals");
    write_in(
        &dir,
        "internals.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         class Field extends HTMLElement {\n\
           static formAssociated = true;\n\
           constructor() { super(); this.internals = this.attachInternals(); }\n\
         }\n\
         customElements.define('x-field', Field);\n\
         customElements.define('x-plain', class extends HTMLElement {});\n\
         function form() {\n\
           const element = document.createElement('form');\n\
           const field = document.createElement('x-field');\n\
           field.setAttribute('name', 'chosen');\n\
           element.append(field);\n\
           document.body.append(element);\n\
           return [element, field];\n\
         }\n\
         test('a form-associated custom element joins its form and its data', () => {\n\
           const [owner, field] = form();\n\
           assertEquals([field.internals.form === owner, owner.elements.length], [true, 1]);\n\
           field.internals.setFormValue('picked');\n\
           assertEquals(Array.from(new FormData(owner).entries()), [['chosen', 'picked']]);\n\
           field.internals.setFormValue(null);\n\
           assertEquals(Array.from(new FormData(owner).entries()), []);\n\
         });\n\
         test('the element decides its own validity', () => {\n\
           const [owner, field] = form();\n\
           let invalid = 0;\n\
           field.addEventListener('invalid', () => invalid++);\n\
           assertEquals([field.internals.validity.valid, owner.checkValidity()], [true, true]);\n\
           field.internals.setValidity({ valueMissing: true }, 'pick something');\n\
           assertEquals([field.internals.validity.valid, field.internals.validity.valueMissing, field.internals.validationMessage], [false, true, 'pick something']);\n\
           assertEquals([field.internals.checkValidity(), invalid, owner.checkValidity()], [false, 1, false]);\n\
           field.internals.setValidity({});\n\
           assertEquals([field.internals.validity.valid, owner.checkValidity()], [true, true]);\n\
           assertEquals(field.internals.validity === field.internals.validity, true);\n\
         });\n\
         test('custom states drive the :state() selector', () => {\n\
           const [owner, field] = form();\n\
           field.internals.states.add('loading');\n\
           assertEquals([field.internals.states.size, owner.querySelectorAll('x-field:state(loading)').length], [1, 1]);\n\
           field.internals.states.delete('loading');\n\
           assertEquals([owner.querySelectorAll('x-field:state(loading)').length, owner.querySelectorAll('x-field:not(:state(loading))').length], [0, 1]);\n\
         });\n\
         test('internals reach a closed root and refuse the wrong element', () => {\n\
           const host = document.createElement('x-field');\n\
           const shadow = host.attachShadow({ mode: 'closed' });\n\
           assertEquals([host.shadowRoot, host.internals.shadowRoot === shadow], [null, true]);\n\
           const name = callback => { try { callback(); return 'ok'; } catch (error) { return error.name; } };\n\
           assertEquals(name(() => document.createElement('x-plain').attachInternals().form), 'NotSupportedError');\n\
           assertEquals(name(() => document.createElement('div').attachInternals()), 'NotSupportedError');\n\
           assertEquals(name(() => host.attachInternals()), 'NotSupportedError');\n\
           assertEquals(name(() => host.internals.setValidity({ valueMissing: true })), 'TypeError');\n\
         });\n\
         test('a form reset reaches the element', () => {\n\
           const [owner, field] = form();\n\
           let resets = 0;\n\
           field.formResetCallback = () => resets++;\n\
           owner.reset();\n\
           assertEquals(resets, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom element internals");
    assert!(
        ran.status.success(),
        "DOM element-internals test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_has_relative_descendant_and_sibling_forms() {
    let dir = build_dir("t_test_dom_selector_has");
    write_in(
        &dir,
        "has.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('has evaluates relative selectors from each candidate', () => {\n\
           document.body.innerHTML = '<section id=one><span class=direct></span><div><i class=deep></i></div></section><section id=two></section><aside class=notice></aside><article id=three></article>';
           const one = document.querySelector('#one'); const two = document.querySelector('#two'); const three = document.querySelector('#three');\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(> .direct)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(.deep)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(> .missing, .deep)')), [one]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(+ aside.notice)')), [two]);\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(~ article)')), [one, two]);\n\
           two.appendChild(document.createElement('i')).className = 'deep';\n\
           assertEquals(Array.from(document.querySelectorAll('section:has(.deep)')), [one, two]);\n\
           assertEquals(three.matches(':has(+ .missing)'), false);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM has selector test");
    assert!(
        ran.status.success(),
        "has selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_type_positions_and_nth_formulas() {
    let dir = build_dir("t_test_dom_selector_nth");
    write_in(
        &dir,
        "nth.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('type positions and An+B formulas', () => {\n\
           document.body.innerHTML = '<ul><li id=one></li><li id=two></li><div></div><li id=three></li><li id=four></li><li id=five></li></ul>';\n\
           const list = document.querySelector('ul'); const one = document.querySelector('#one'); const three = document.querySelector('#three'); const five = document.querySelector('#five');\n\
           assertEquals(Array.from(list.querySelectorAll('li:first-of-type')), [one]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:last-of-type')), [five]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-child(2n + 1)')), [one, document.querySelector('#four')]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-of-type(2n + 1)')), [one, three, five]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-of-type(even)')), [document.querySelector('#two'), document.querySelector('#four')]);\n\
           assertEquals(Array.from(list.querySelectorAll('li:nth-last-of-type(2)')), [document.querySelector('#four')]);\n\
           assertThrows(() => list.querySelector(':nth-child(two)'), SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM nth selector test");
    assert!(
        ran.status.success(),
        "nth selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_selectors_match_dynamic_form_state() {
    let dir = build_dir("t_test_dom_selector_form_state");
    write_in(
        &dir,
        "form-state.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form state selectors follow reflected attributes and defaults', () => {\n\
           const checked = document.createElement('input'); checked.type = 'checkbox'; checked.checked = true;\n\
           const disabled = document.createElement('button'); disabled.disabled = true;\n\
           const required = document.createElement('input'); required.required = true;\n\
           const optional = document.createElement('input'); const plain = document.createElement('div');\n\
           document.body.append(checked, disabled, required, optional, plain);\n\
           assertEquals(Array.from(document.querySelectorAll(':checked')), [checked]);\n\
           assertEquals(Array.from(document.querySelectorAll(':disabled')), [disabled]);\n\
           assertEquals(Array.from(document.querySelectorAll(':enabled')), [checked, required, optional]);\n\
           assertEquals(Array.from(document.querySelectorAll(':required')), [required]);\n\
           assertEquals(Array.from(document.querySelectorAll(':optional')), [checked, optional]);\n\
           checked.checked = false; disabled.disabled = false; required.required = false;\n\
           assertEquals(document.querySelectorAll(':checked').length, 0);\n\
           assertEquals(document.querySelectorAll(':disabled').length, 0);\n\
           assertEquals(Array.from(document.querySelectorAll(':optional')), [checked, required, optional]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form-state selector test");
    assert!(
        ran.status.success(),
        "form-state selector test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_text_inputs_keep_and_clamp_selection_ranges() {
    let dir = build_dir("t_test_dom_input_selection");
    write_in(
        &dir,
        "selection.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('text inputs expose browser selection state', () => {\n\
           const input = document.createElement('input'); input.value = 'hello';\n\
           input.setSelectionRange(1, 4, 'backward');\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [1, 4, 'backward']);\n\
           input.value = 'hi';\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [2, 2, 'none']);\n\
           input.setSelectionRange(-1, 99); assertEquals([input.selectionStart, input.selectionEnd], [0, 2]);\n\
           input.type = 'number';\n\
           assertEquals([input.selectionStart, input.selectionEnd, input.selectionDirection], [null, null, null]);\n\
           assertThrows(() => input.setSelectionRange(0, 1), DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom input selection");
    assert!(
        ran.status.success(),
        "DOM input-selection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `select()`, `setRangeText()` and the selection members on `<input>` and
/// `<textarea>` alike, each the interface's own prototype member — which is
/// where Testing Library's user-event looks for them.
#[test]
fn test_dom_text_controls_select_and_replace_ranges() {
    run_dom_test(
        "t_test_dom_text_controls",
        "controls.test.mjs",
        r#"import { test, assertEquals, assertThrows } from 'runtime:test';
test('each member is the interface prototype own', () => {
  for (const Class of [HTMLInputElement, HTMLTextAreaElement]) {
    for (const name of ['select', 'setSelectionRange', 'setRangeText']) {
      assertEquals(typeof Object.getOwnPropertyDescriptor(Class.prototype, name)?.value, 'function', `${Class.name}.${name}`);
    }
    for (const name of ['value', 'selectionStart', 'selectionEnd', 'selectionDirection']) {
      assertEquals(typeof Object.getOwnPropertyDescriptor(Class.prototype, name)?.set, 'function', `${Class.name}.${name}`);
    }
  }
});
test('select() selects everything, and fires select as a task', async () => {
  for (const tag of ['input', 'textarea']) {
    const control = document.createElement(tag); control.value = 'hello';
    document.body.append(control);
    let fired = 0;
    control.addEventListener('select', (event) => { fired++; assertEquals(event.bubbles, true); });
    control.select();
    assertEquals([control.selectionStart, control.selectionEnd, control.selectionDirection], [0, 5, 'none'], tag);
    assertEquals(fired, 0, 'not synchronously');
    await new Promise((resolve) => setTimeout(resolve, 0));
    assertEquals(fired, 1, tag);
    control.remove();
  }
});
test('select() is a no-op where it has nothing to select', () => {
  for (const type of ['number', 'email', 'checkbox', 'hidden']) {
    const input = document.createElement('input'); input.type = type;
    input.select();
    assertEquals(input.selectionStart, null, type);
  }
});
test('a textarea keeps a selection, and its value moves the cursor', () => {
  const area = document.createElement('textarea'); area.value = 'one\ntwo';
  assertEquals([area.selectionStart, area.selectionEnd, area.textLength], [7, 7, 7]);
  area.setSelectionRange(2, 5, 'backward');
  assertEquals([area.selectionStart, area.selectionEnd, area.selectionDirection], [2, 5, 'backward']);
  area.selectionStart = 6;
  assertEquals([area.selectionStart, area.selectionEnd], [6, 6]);
  area.value = 'x';
  assertEquals([area.selectionStart, area.selectionEnd, area.selectionDirection], [1, 1, 'none']);
});
test('setRangeText() replaces and places the selection by mode', () => {
  const input = document.createElement('input'); input.value = 'abcdef';
  input.setSelectionRange(2, 4);
  input.setRangeText('XY');
  assertEquals([input.value, input.selectionStart, input.selectionEnd], ['abXYef', 2, 4]);
  input.setRangeText('123', 0, 2, 'select');
  assertEquals([input.value, input.selectionStart, input.selectionEnd], ['123XYef', 0, 3]);
  input.setRangeText('', 0, 3, 'start');
  assertEquals([input.value, input.selectionStart, input.selectionEnd], ['XYef', 0, 0]);
  input.setRangeText('!', 4, 4, 'end');
  assertEquals([input.value, input.selectionStart, input.selectionEnd], ['XYef!', 5, 5]);
  input.setSelectionRange(3, 5);
  input.setRangeText('--', 0, 1, 'preserve');
  assertEquals([input.value, input.selectionStart, input.selectionEnd], ['--Yef!', 4, 6]);
  assertThrows(() => input.setRangeText('z', 3, 1), DOMException);
  input.type = 'email';
  assertThrows(() => input.setRangeText('z'), DOMException);
});
"#,
        "DOM text-control selection test did not run",
    );
}

#[test]
fn test_dom_selector_pseudo_classes_reject_malformed_and_unsupported_syntax() {
    let dir = build_dir("t_test_dom_selector_pseudo_errors");
    write_in(
        &dir,
        "pseudo-errors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('pseudo parsing stays strict', () => {\n\
           assertThrows(() => document.querySelector(':is()'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':has(>)'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':not(.one'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':nonsense'), SyntaxError);\n\
           assertThrows(() => document.querySelector('::before'), SyntaxError);\n\
           assertThrows(() => document.querySelector(':dir(ltr)'), SyntaxError);\n\
         });\n\
         test('a pseudo-class with no answer here parses and matches nothing', () => {\n\
           document.body.innerHTML = '<a href=\"#x\">link</a><input placeholder=p><input value=v>';\n\
           const names = selector => Array.from(document.body.querySelectorAll(selector), element => element.localName);\n\
           assertEquals([names(':hover'), names(':active'), names(':visited'), names(':focus-visible')], [[], [], [], []]);\n\
           assertEquals(names(':any-link'), ['a']);\n\
           assertEquals(names('[placeholder]:placeholder-shown'), ['input']);\n\
           assertEquals(names('input:read-write').length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM pseudo selector error test");
    assert!(
        ran.status.success(),
        "pseudo selector error test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reflects_common_attributes_and_form_defaults() {
    let dir = build_dir("t_test_dom_elements");
    write_in(
        &dir,
        "elements.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('reflection', () => {\n\
           const input = document.createElement('input');\n\
           input.id = 'email'; input.className = 'field'; input.disabled = true; input.value = 42;\n\
           let refused = null; try { input.size = 0; } catch (error) { refused = error.name; }\n\
           assertEquals(input instanceof HTMLElement, true);\n\
           assertEquals(input instanceof HTMLInputElement, true);\n\
           assertEquals(input.getAttribute('id'), 'email');\n\
           assertEquals(input.getAttribute('class'), 'field');\n\
           assertEquals(input.disabled, true);\n\
           assertEquals(input.hasAttribute('disabled'), true);\n\
           assertEquals([refused, input.size], ['IndexSizeError', 20]);\n\
           assertEquals(input.value, '42');\n\
           input.disabled = false;\n\
           assertEquals(input.hasAttribute('disabled'), false);\n\
           const label = document.createElement('label'); label.htmlFor = 'email';\n\
           assertEquals(label.getAttribute('for'), 'email');\n\
           const dialog = document.createElement('dialog'); dialog.open = true;\n\
           assertEquals([dialog instanceof HTMLDialogElement, dialog.open, dialog.getAttribute('open')], [true, true, '']);\n\
           dialog.open = false; assertEquals([dialog.open, dialog.hasAttribute('open')], [false, false]);\n\
         });\n\
         test('click defaults', () => {\n\
           const form = document.createElement('form');\n\
           const checkbox = document.createElement('input'); checkbox.type = 'checkbox';\n\
           const radioOne = document.createElement('input'); radioOne.type = 'radio'; radioOne.name = 'choice';\n\
           const radioTwo = document.createElement('input'); radioTwo.type = 'radio'; radioTwo.name = 'choice';\n\
           const submit = document.createElement('button');\n\
           form.append(checkbox, radioOne, radioTwo, submit); document.body.appendChild(form);\n\
           checkbox.click(); assertEquals(checkbox.checked, true);\n\
           radioOne.click(); radioTwo.click(); assertEquals(radioOne.checked, false); assertEquals(radioTwo.checked, true);\n\
           let submitted = 0; form.addEventListener('submit', () => { submitted += 1; }); submit.click();\n\
           assertEquals(submitted, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom elements");
    assert!(
        ran.status.success(),
        "DOM element test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_select_controls_choose_values_and_expose_live_options() {
    let dir = build_dir("t_test_dom_select_controls");
    write_in(
        &dir,
        "select.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('single selects expose fallback values and selection state', () => {\n\
           const select = document.createElement('select'); const one = document.createElement('option'); const two = document.createElement('option');\n\
           one.textContent = 'one'; two.value = 'second'; two.textContent = 'two'; select.append(one, two); document.body.appendChild(select);\n\
           const options = select.options; assertEquals(select instanceof HTMLSelectElement, true); assertEquals(one instanceof HTMLOptionElement, true);\n\
           assertEquals([select.length, select.selectedIndex, select.value, Array.from(select.selectedOptions)], [2, 0, 'one', [one]]);\n\
           select.value = 'second'; assertEquals([select.selectedIndex, select.value, one.selected, two.selected], [1, 'second', false, true]);\n\
           select.selectedIndex = -1; assertEquals([select.selectedIndex, select.value, Array.from(select.selectedOptions)], [-1, '', []]);\n\
           const three = document.createElement('option'); three.value = 'third'; select.add(three, 1); assertEquals([options.length, options.item(1)], [3, three]);\n\
           select.remove(1); assertEquals(options.length, 2);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM select-control test");
    assert!(
        ran.status.success(),
        "select-control test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_multiple_selects_keep_each_selected_option() {
    let dir = build_dir("t_test_dom_multiple_select");
    write_in(
        &dir,
        "multiple.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('multiple select does not deselect sibling options', () => {\n\
           const select = document.createElement('select'); select.multiple = true;\n\
           const first = document.createElement('option'); first.value = 'first'; const second = document.createElement('option'); second.value = 'second'; const third = document.createElement('option'); third.value = 'third';\n\
           select.append(first, second, third); document.body.appendChild(select); first.selected = true; third.selected = true;\n\
           assertEquals([select.selectedIndex, select.value, Array.from(select.selectedOptions)], [0, 'first', [first, third]]);\n\
           second.selected = true; assertEquals(Array.from(select.selectedOptions), [first, second, third]);\n\
           select.length = 2; assertEquals([select.options.length, Array.from(select.selectedOptions)], [2, [first, second]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM multiple-select test");
    assert!(
        ran.status.success(),
        "multiple-select test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_select_and_textarea_defaults() {
    let dir = build_dir("t_test_dom_form_reset_controls");
    write_in(
        &dir,
        "reset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form controls retain defaults and reset dirty values', () => {\n\
           document.body.innerHTML = '<form><select name=choice><option value=one>one</option><option value=two selected>two</option></select><textarea name=note>initial</textarea></form>';\n\
           const form = document.querySelector('form'); const select = document.querySelector('select'); const textarea = document.querySelector('textarea'); const controls = form.elements;\n\
           assertEquals([form instanceof HTMLFormElement, textarea instanceof HTMLTextAreaElement, select.value, textarea.defaultValue, textarea.value, controls.length], [true, true, 'two', 'initial', 'initial', 2]);\n\
           select.value = 'one'; textarea.value = 'changed'; assertEquals([select.value, textarea.value], ['one', 'changed']);\n\
           form.reset(); assertEquals([select.value, textarea.value, Array.from(controls)], ['two', 'initial', [select, textarea]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form reset controls test");
    assert!(
        ran.status.success(),
        "form reset controls test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_controls_associate_by_ancestor_or_form_attribute() {
    let dir = build_dir("t_test_dom_form_ownership");
    write_in(
        &dir,
        "ownership.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form ownership includes associated controls outside the form', () => {\n\
           const form = document.createElement('form'); form.id = 'profile'; const inside = document.createElement('input'); inside.name = 'inside';\n\
           const outside = document.createElement('textarea'); outside.name = 'outside'; outside.setAttribute('form', 'profile');\n\
           const foreign = document.createElement('input'); foreign.name = 'foreign'; foreign.setAttribute('form', 'other');\n\
           form.appendChild(inside); document.body.append(form, outside, foreign); const controls = form.elements;\n\
           assertEquals([inside.form, outside.form, foreign.form], [form, form, null]);\n\
           assertEquals(Array.from(controls), [inside, outside]);\n\
           const button = document.createElement('button'); button.setAttribute('form', 'profile'); let submitted = 0; form.addEventListener('submit', (event) => { submitted += 1; assertEquals(event.submitter, button); });\n\
           document.body.appendChild(button); button.click(); assertEquals(submitted, 1);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form ownership test");
    assert!(
        ran.status.success(),
        "form ownership test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_serializes_successful_controls_in_tree_order() {
    let dir = build_dir("t_test_dom_form_data");
    write_in(
        &dir,
        "form-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData(form) includes successful controls only', () => {\n\
           const form = document.createElement('form'); form.id = 'survey';\n\
           const text = document.createElement('input'); text.name = 'name'; text.value = 'Ada';\n\
           const checked = document.createElement('input'); checked.type = 'checkbox'; checked.name = 'newsletter'; checked.value = 'yes'; checked.checked = true;\n\
           const unchecked = document.createElement('input'); unchecked.type = 'checkbox'; unchecked.name = 'skip'; unchecked.value = 'no';\n\
           const select = document.createElement('select'); select.multiple = true; select.name = 'tag'; const one = document.createElement('option'); one.value = 'one'; one.selected = true; const two = document.createElement('option'); two.value = 'two'; two.selected = true; select.append(one, two);\n\
           const note = document.createElement('textarea'); note.name = 'note'; note.value = 'hello'; const disabled = document.createElement('input'); disabled.name = 'disabled'; disabled.value = 'never'; disabled.disabled = true;\n\
           form.append(text, checked, unchecked, select, note, disabled); document.body.appendChild(form);\n\
           assertEquals(Array.from(new FormData(form)), [['name', 'Ada'], ['newsletter', 'yes'], ['tag', 'one'], ['tag', 'two'], ['note', 'hello']]);\n\
           assertEquals(Array.from(new FormData()), []);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM FormData test");
    assert!(
        ran.status.success(),
        "FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_includes_externally_associated_controls() {
    let dir = build_dir("t_test_dom_form_data_external");
    write_in(
        &dir,
        "form-data-external.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData follows form ownership rather than subtree only', () => {\n\
           const form = document.createElement('form'); form.id = 'checkout'; const local = document.createElement('input'); local.name = 'local'; local.value = 'one'; form.appendChild(local);\n\
           const external = document.createElement('select'); external.name = 'external'; external.setAttribute('form', 'checkout'); const option = document.createElement('option'); option.value = 'two'; external.appendChild(option);\n\
           document.body.append(form, external); assertEquals(Array.from(new FormData(form)), [['local', 'one'], ['external', 'two']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn external DOM FormData test");
    assert!(
        ran.status.success(),
        "external FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_controls_report_common_constraint_validity_flags() {
    let dir = build_dir("t_test_dom_constraint_flags");
    write_in(
        &dir,
        "validity.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('required pattern type range and length constraints are distinct', () => {\n\
           const required = document.createElement('input'); required.required = true;\n\
           const pattern = document.createElement('input'); pattern.pattern = '[0-9]{3}'; pattern.value = 'no';\n\
           const email = document.createElement('input'); email.type = 'email'; email.value = 'not-an-email';\n\
           const number = document.createElement('input'); number.type = 'number'; number.min = '2'; number.max = '4'; number.value = '5';\n\
           const length = document.createElement('input'); length.minLength = 3; length.maxLength = 4; length.value = 'ab';\n\
           assertEquals([required.validity.valueMissing, pattern.validity.patternMismatch, email.validity.typeMismatch, number.validity.rangeOverflow, length.validity.tooShort], [true, true, true, true, true]);\n\
           assertEquals([required.checkValidity(), required.validationMessage], [false, 'Constraints not satisfied']);\n\
           required.value = 'ok'; pattern.value = '123'; email.value = 'ada@example.test'; number.value = '3'; length.value = 'abcd';\n\
           assertEquals([required.validity.valid, pattern.validity.valid, email.validity.valid, number.validity.valid, length.validity.valid], [true, true, true, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM constraint flag test");
    assert!(
        ran.status.success(),
        "constraint flag test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_constraint_validation_dispatches_invalid_and_honors_custom_errors() {
    let dir = build_dir("t_test_dom_constraint_events");
    write_in(
        &dir,
        "invalid.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('invalid events and custom messages follow checkValidity', () => {\n\
           const input = document.createElement('input'); input.required = true; document.body.appendChild(input); let invalid = 0; let bubbled = 0;\n\
           input.addEventListener('invalid', (event) => { invalid += 1; assertEquals(event.bubbles, false); }); document.body.addEventListener('invalid', () => { bubbled += 1; });\n\
           assertEquals(input.reportValidity(), false); assertEquals([invalid, bubbled], [1, 0]);\n\
           input.value = 'present'; input.setCustomValidity('server rejected it');\n\
           assertEquals([input.validity.customError, input.validity.valid, input.validationMessage, input.checkValidity()], [true, false, 'server rejected it', false]);\n\
           input.setCustomValidity(''); assertEquals(input.checkValidity(), true);\n\
           input.disabled = true; input.value = ''; assertEquals([input.willValidate, input.checkValidity()], [false, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM constraint event test");
    assert!(
        ran.status.success(),
        "constraint event test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_validates_before_emitting_submit() {
    let dir = build_dir("t_test_dom_request_submit");
    write_in(
        &dir,
        "request-submit.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('requestSubmit stops on invalid controls then names its submitter', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.required = true; const button = document.createElement('button'); form.append(input, button); document.body.appendChild(form);\n\
           let invalid = 0; let submitted = 0; input.addEventListener('invalid', () => { invalid += 1; }); form.addEventListener('submit', (event) => { submitted += 1; assertEquals(event.submitter, button); });\n\
           form.requestSubmit(button); assertEquals([invalid, submitted], [1, 0]);\n\
           input.value = 'ready'; form.requestSubmit(button); assertEquals([invalid, submitted], [1, 1]);\n\
           input.setCustomValidity('no'); assertEquals(form.checkValidity(), false); input.setCustomValidity(''); assertEquals(form.reportValidity(), true);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM request-submit test");
    assert!(
        ran.status.success(),
        "request-submit test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_disabled_fieldsets_exclude_descendants_from_validation_and_form_data() {
    let dir = build_dir("t_test_dom_fieldset_disabled");
    write_in(
        &dir,
        "fieldset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('disabled fieldsets suppress descendant controls', () => {\n\
           const form = document.createElement('form'); const fieldset = document.createElement('fieldset'); fieldset.disabled = true; const blocked = document.createElement('input'); blocked.name = 'blocked'; blocked.required = true; blocked.value = '';\n\
           const active = document.createElement('input'); active.name = 'active'; active.value = 'yes'; fieldset.appendChild(blocked); form.append(fieldset, active); document.body.appendChild(form);\n\
           assertEquals([fieldset instanceof HTMLFieldSetElement, blocked.willValidate, blocked.checkValidity(), form.checkValidity(), Array.from(new FormData(form))], [true, false, true, true, [['active', 'yes']]]);\n\
           fieldset.disabled = false; blocked.value = 'now'; assertEquals([blocked.willValidate, form.checkValidity(), Array.from(new FormData(form))], [true, true, [['blocked', 'now'], ['active', 'yes']]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM fieldset disabled test");
    assert!(
        ran.status.success(),
        "fieldset disabled test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_disabled_optgroups_exclude_selected_options_from_form_data() {
    let dir = build_dir("t_test_dom_optgroup_disabled");
    write_in(
        &dir,
        "optgroup.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('disabled optgroups leave selected options visible but unsuccessful', () => {\n\
           const form = document.createElement('form'); const select = document.createElement('select'); select.name = 'choice'; select.multiple = true;\n\
           const blockedGroup = document.createElement('optgroup'); blockedGroup.disabled = true; blockedGroup.label = 'blocked'; const blocked = document.createElement('option'); blocked.value = 'blocked'; blocked.selected = true; blockedGroup.appendChild(blocked);\n\
           const activeGroup = document.createElement('optgroup'); const active = document.createElement('option'); active.value = 'active'; active.selected = true; activeGroup.appendChild(active);\n\
           select.append(blockedGroup, activeGroup); form.appendChild(select); document.body.appendChild(form);\n\
           assertEquals([blockedGroup instanceof HTMLOptGroupElement, Array.from(select.selectedOptions), Array.from(new FormData(form))], [true, [blocked, active], [['choice', 'active']]]);\n\
           blockedGroup.disabled = false; assertEquals(Array.from(new FormData(form)), [['choice', 'blocked'], ['choice', 'active']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM optgroup disabled test");
    assert!(
        ran.status.success(),
        "optgroup disabled test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_elements_include_live_fieldsets_and_associated_controls() {
    let dir = build_dir("t_test_dom_fieldset_collections");
    write_in(
        &dir,
        "fieldset-collections.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('form elements remain live across fieldset mutations', () => {\n\
           const form = document.createElement('form'); form.id = 'settings'; const fieldset = document.createElement('fieldset'); const inside = document.createElement('input'); fieldset.appendChild(inside); form.appendChild(fieldset);\n\
           const external = document.createElement('textarea'); external.setAttribute('form', 'settings'); document.body.append(form, external); const elements = form.elements;\n\
           assertEquals(Array.from(elements), [fieldset, inside, external]);\n\
           inside.remove(); assertEquals(Array.from(elements), [fieldset, external]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM fieldset collection test");
    assert!(
        ran.status.success(),
        "fieldset collection test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_radio_clicks_are_exclusive_within_one_form_only() {
    let dir = build_dir("t_test_dom_radio_form_scope");
    write_in(
        &dir,
        "radios.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('same-name radios do not cross form ownership', () => {\n\
           const one = document.createElement('form'); const two = document.createElement('form');\n\
           const first = document.createElement('input'); first.type = 'radio'; first.name = 'choice'; const second = document.createElement('input'); second.type = 'radio'; second.name = 'choice'; const other = document.createElement('input'); other.type = 'radio'; other.name = 'choice';\n\
           one.append(first, second); two.appendChild(other); document.body.append(one, two);\n\
           first.click(); other.click(); assertEquals([first.checked, second.checked, other.checked], [true, false, true]);\n\
           second.click(); assertEquals([first.checked, second.checked, other.checked], [false, true, true]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM radio form-scope test");
    assert!(
        ran.status.success(),
        "radio form-scope test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_labels_resolve_controls_and_expose_live_control_labels() {
    let dir = build_dir("t_test_dom_label_association");
    write_in(
        &dir,
        "labels.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('explicit and nested labels share control association', () => {\n\
           const input = document.createElement('input'); input.id = 'email'; const explicit = document.createElement('label'); explicit.htmlFor = 'email'; const nested = document.createElement('label'); nested.appendChild(input); document.body.append(explicit, nested);\n\
           assertEquals([explicit.control, nested.control, Array.from(input.labels)], [input, input, [explicit, nested]]);\n\
           let clicks = 0; input.addEventListener('click', () => { clicks += 1; }); explicit.click(); nested.click(); assertEquals(clicks, 2);\n\
           const extra = document.createElement('label'); extra.htmlFor = 'email'; document.body.appendChild(extra); assertEquals(Array.from(input.labels), [explicit, nested, extra]);\n\
           extra.remove(); assertEquals(Array.from(input.labels), [explicit, nested]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM label association test");
    assert!(
        ran.status.success(),
        "label association test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_checkbox_and_radio_form_data_default_to_on() {
    let dir = build_dir("t_test_dom_checkable_default_value");
    write_in(
        &dir,
        "checkable-values.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('successful checkable controls default their value to on', () => {\n\
           const form = document.createElement('form'); const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.name = 'check'; checkbox.checked = true; const radio = document.createElement('input'); radio.type = 'radio'; radio.name = 'radio'; radio.checked = true;\n\
           form.append(checkbox, radio); document.body.appendChild(form);\n\
           assertEquals([checkbox.value, radio.value, Array.from(new FormData(form))], ['on', 'on', [['check', 'on'], ['radio', 'on']]]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM checkable value test");
    assert!(
        ran.status.success(),
        "checkable value test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_dirty_input_values_to_default_values() {
    let dir = build_dir("t_test_dom_input_value_reset");
    write_in(
        &dir,
        "input-values.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('value changes are dirty state rather than attribute writes', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.defaultValue = 'markup'; form.appendChild(input); document.body.appendChild(form);\n\
           input.value = 'typed'; assertEquals([input.value, input.defaultValue, input.getAttribute('value')], ['typed', 'markup', 'markup']);\n\
           form.reset(); assertEquals(input.value, 'markup');\n\
           input.defaultValue = 'revised'; assertEquals([input.value, input.defaultValue], ['revised', 'revised']);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM input value reset test");
    assert!(
        ran.status.success(),
        "input value reset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_reset_restores_checkable_defaults_without_rewriting_attributes() {
    let dir = build_dir("t_test_dom_checked_reset");
    write_in(
        &dir,
        "checked-reset.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('checked state is independent from defaultChecked', () => {\n\
           const form = document.createElement('form'); const checked = document.createElement('input'); checked.type = 'checkbox'; checked.defaultChecked = true; const clear = document.createElement('input'); clear.type = 'checkbox'; form.append(checked, clear); document.body.appendChild(form);\n\
           checked.checked = false; clear.checked = true; assertEquals([checked.checked, checked.defaultChecked, clear.checked, clear.defaultChecked, checked.hasAttribute('checked')], [false, true, true, false, true]);\n\
           form.reset(); assertEquals([checked.checked, clear.checked], [true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM checked reset test");
    assert!(
        ran.status.success(),
        "checked reset test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_data_uses_dirty_input_values_before_and_after_reset() {
    let dir = build_dir("t_test_dom_dirty_form_data");
    write_in(
        &dir,
        "dirty-form-data.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('FormData reads current dirty values', () => {\n\
           const form = document.createElement('form'); const input = document.createElement('input'); input.name = 'message'; input.defaultValue = 'initial'; form.appendChild(input); document.body.appendChild(form);\n\
           input.value = 'edited'; assertEquals(Array.from(new FormData(form)), [['message', 'edited']]);\n\
           form.reset(); assertEquals(Array.from(new FormData(form)), [['message', 'initial']]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM dirty FormData test");
    assert!(
        ran.status.success(),
        "dirty FormData test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_number_inputs_normalize_values_and_expose_numeric_state() {
    let dir = build_dir("t_test_dom_number_inputs");
    write_in(
        &dir,
        "number.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('number inputs sanitize and convert values', () => {\n\
           const input = document.createElement('input'); input.type = 'number';\n\
           input.value = '004.50'; assertEquals([input.value, input.valueAsNumber], ['004.50', 4.5]);\n\
           input.value = 'not-a-number'; assertEquals([input.value, Number.isNaN(input.valueAsNumber)], ['', true]);\n\
           input.valueAsNumber = 12.25; assertEquals([input.value, input.valueAsNumber], ['12.25', 12.25]);\n\
           input.valueAsNumber = NaN; assertEquals(input.value, '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM number input test");
    assert!(
        ran.status.success(),
        "number input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_date_inputs_validate_calendar_values_and_dates() {
    let dir = build_dir("t_test_dom_date_inputs");
    write_in(
        &dir,
        "date.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('date inputs reject impossible dates and use UTC dates', () => {\n\
           const input = document.createElement('input'); input.type = 'date'; input.value = '2024-02-29';\n\
           assertEquals([input.value, input.valueAsDate.toISOString(), input.valueAsNumber], ['2024-02-29', '2024-02-29T00:00:00.000Z', Date.UTC(2024, 1, 29)]);\n\
           input.value = '2023-02-29'; assertEquals([input.value, input.valueAsDate, Number.isNaN(input.valueAsNumber)], ['', null, true]);\n\
           input.valueAsDate = new Date('2025-03-04T12:00:00Z'); assertEquals(input.value, '2025-03-04');\n\
           input.valueAsDate = null; assertEquals(input.value, '');\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM date input test");
    assert!(
        ran.status.success(),
        "date input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_value_as_accessors_reject_unsupported_input_types() {
    let dir = build_dir("t_test_dom_value_as_type_errors");
    write_in(
        &dir,
        "value-as-errors.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('valueAs accessors are strict outside number and date types', () => {\n\
           const text = document.createElement('input'); text.type = 'text'; text.value = 'plain';\n\
           assertEquals([Number.isNaN(text.valueAsNumber), text.valueAsDate], [true, null]);\n\
           assertThrows(() => { text.valueAsNumber = 1; }, DOMException); assertThrows(() => { text.valueAsDate = new Date(); }, DOMException);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM valueAs type error test");
    assert!(
        ran.status.success(),
        "valueAs type error test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_buttons_honor_form_settings_and_validation_bypasses() {
    let dir = build_dir("t_test_dom_submit_settings");
    write_in(
        &dir,
        "submit.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
      test('submit settings and bypasses', () => {\n\
        const form = document.createElement('form'); form.action = '/send'; form.method = 'POST'; form.enctype = 'multipart/form-data'; form.target = '_blank';\n\
        const input = document.createElement('input'); input.required = true; const button = document.createElement('button'); form.append(input, button); document.body.appendChild(form); let submits = 0; form.addEventListener('submit', () => { submits += 1; });\n\
        button.click(); assertEquals(submits, 0); button.formNoValidate = true; button.click(); assertEquals(submits, 1);\n\
        button.formNoValidate = false; form.noValidate = true; form.requestSubmit(); assertEquals(submits, 2);\n\
        assertEquals([form.action, form.method, form.enctype, form.target], ['http://localhost/send', 'post', 'multipart/form-data', '_blank']);\n\
      });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit settings test");
    assert!(
        ran.status.success(),
        "submit settings test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_action_resolves_against_the_current_location() {
    let dir = build_dir("t_test_dom_form_action_resolution");
    write_in(
        &dir,
        "form-action.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('form actions resolve from the in-memory location', () => {
  const form = document.createElement('form'); form.action = '/send';
  assertEquals([form.getAttribute('action'), form.action], ['/send', 'http://localhost/send']);
  history.pushState(null, '', '/nested/page'); form.action = 'next';
  assertEquals(form.action, 'http://localhost/nested/next');
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form action test");
    assert!(
        ran.status.success(),
        "form action test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submitter_actions_resolve_and_preserve_their_attributes() {
    let dir = build_dir("t_test_dom_submitter_action_resolution");
    write_in(
        &dir,
        "submitter-action.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submitter actions resolve without rewriting attributes', () => {
  const button = document.createElement('button'); const input = document.createElement('input'); input.type = 'submit';
  button.formAction = '/button'; input.formAction = 'input';
  assertEquals([button.getAttribute('formaction'), button.formAction], ['/button', 'http://localhost/button']);
  assertEquals([input.getAttribute('formaction'), input.formAction], ['input', 'http://localhost/input']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submitter action test");
    assert!(
        ran.status.success(),
        "submitter action test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_form_action_defaults_to_the_current_location() {
    let dir = build_dir("t_test_dom_form_action_default");
    write_in(
        &dir,
        "form-action-default.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('missing form action uses the current location', () => {
  const form = document.createElement('form'); const submit = document.createElement('button');
  assertEquals([form.action, submit.formAction], ['http://localhost/', 'http://localhost/']);
  history.replaceState(null, '', '/current'); assertEquals([form.action, submit.formAction], ['http://localhost/current', 'http://localhost/current']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM form action default test");
    assert!(
        ran.status.success(),
        "form action default test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_uses_default_submitter_and_rejects_foreign_button() {
    let dir = build_dir("t_test_dom_submit_default");
    write_in(
        &dir,
        "submit-default.test.mjs",
        r#"import { test, assertEquals, assertThrows } from 'runtime:test';
test('requestSubmit selects the default submitter', () => {
  const form = document.createElement('form'); const first = document.createElement('button'); const second = document.createElement('button'); form.append(first, second); document.body.appendChild(form);
  let submitter = null; form.addEventListener('submit', (event) => { submitter = event.submitter; }); form.requestSubmit(); assertEquals(submitter, first);
  const foreign = document.createElement('button'); assertThrows(() => form.requestSubmit(foreign), TypeError);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM default submitter test");
    assert!(
        ran.status.success(),
        "default submitter test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_button_respects_cancelled_clicks_and_disabled_state() {
    let dir = build_dir("t_test_dom_submit_click");
    write_in(
        &dir,
        "submit-click.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submit button activation is cancelable and disabled-aware', () => {
  const form = document.createElement('form'); const button = document.createElement('button'); form.appendChild(button); document.body.appendChild(form);
  let submissions = 0; form.addEventListener('submit', () => { submissions += 1; }); button.addEventListener('click', (event) => event.preventDefault(), { once: true });
  button.click(); assertEquals(submissions, 0); button.disabled = true; button.click(); assertEquals(submissions, 0);
  button.disabled = false; button.click(); assertEquals(submissions, 1);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit click test");
    assert!(
        ran.status.success(),
        "submit click test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submit_inputs_validate_and_can_bypass_validation() {
    let dir = build_dir("t_test_dom_submit_input_validation");
    write_in(
        &dir,
        "submit-input-validation.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submit inputs validate and honor formNoValidate', () => {
  const form = document.createElement('form'); const required = document.createElement('input'); required.required = true; const submit = document.createElement('input'); submit.type = 'submit'; form.append(required, submit); document.body.appendChild(form);
  let submitter = null; form.addEventListener('submit', (event) => { submitter = event.submitter; });
  submit.click(); assertEquals(submitter, null);
  submit.formNoValidate = true; submit.click(); assertEquals(submitter, submit);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submit input validation test");
    assert!(
        ran.status.success(),
        "submit input validation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_submitters_reflect_form_override_settings() {
    let dir = build_dir("t_test_dom_submitter_overrides");
    write_in(
        &dir,
        "submitter-overrides.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('submitters reflect their form overrides', () => {
  const button = document.createElement('button'); const input = document.createElement('input'); input.type = 'submit';
  button.formAction = '/button'; button.formMethod = 'post'; button.formEnctype = 'text/plain'; button.formTarget = '_blank'; button.formNoValidate = true;
  input.formAction = '/input'; input.formMethod = 'dialog'; input.formEnctype = 'multipart/form-data'; input.formTarget = 'result'; input.formNoValidate = true;
  assertEquals([button.formAction, button.formMethod, button.formEnctype, button.formTarget, button.formNoValidate], ['http://localhost/button', 'post', 'text/plain', '_blank', true]);
  assertEquals([input.formAction, input.formMethod, input.formEnctype, input.formTarget, input.formNoValidate], ['http://localhost/input', 'dialog', 'multipart/form-data', 'result', true]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM submitter override test");
    assert!(
        ran.status.success(),
        "submitter override test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_request_submit_rejects_non_submit_inputs() {
    let dir = build_dir("t_test_dom_non_submit_request");
    write_in(
        &dir,
        "non-submitter.test.mjs",
        r#"import { test, assertThrows } from 'runtime:test';
test('requestSubmit rejects a non-submit input', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); form.appendChild(text); document.body.appendChild(form);
  assertThrows(() => form.requestSubmit(text), TypeError);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM non-submitter test");
    assert!(
        ran.status.success(),
        "non-submitter test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_button_restores_dirty_control_state() {
    let dir = build_dir("t_test_dom_reset_button");
    write_in(
        &dir,
        "reset-button.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset button restores dirty input state', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); text.defaultValue = 'markup'; const check = document.createElement('input'); check.type = 'checkbox'; check.defaultChecked = true; const reset = document.createElement('button'); reset.type = 'reset'; form.append(text, check, reset); document.body.appendChild(form);
  text.value = 'changed'; check.checked = false; reset.click();
  assertEquals([text.value, check.checked], ['markup', true]);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset button test");
    assert!(
        ran.status.success(),
        "reset button test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_input_restores_select_and_textarea_state() {
    let dir = build_dir("t_test_dom_reset_input");
    write_in(
        &dir,
        "reset-input.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset input restores select and textarea state', () => {
  const form = document.createElement('form'); const select = document.createElement('select'); const first = document.createElement('option'); first.value = 'one'; const second = document.createElement('option'); second.value = 'two'; second.defaultSelected = true; select.append(first, second); const area = document.createElement('textarea'); area.defaultValue = 'markup'; const reset = document.createElement('input'); reset.type = 'reset'; form.append(select, area, reset); document.body.appendChild(form);
  select.value = 'one'; area.value = 'changed'; reset.click();
  assertEquals([select.value, area.value], ['two', 'markup']);
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset input test");
    assert!(
        ran.status.success(),
        "reset input test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_reset_control_honors_cancelled_events_and_disabled_state() {
    let dir = build_dir("t_test_dom_reset_cancellation");
    write_in(
        &dir,
        "reset-cancellation.test.mjs",
        r#"import { test, assertEquals } from 'runtime:test';
test('reset activation respects cancellation and disabled controls', () => {
  const form = document.createElement('form'); const text = document.createElement('input'); text.defaultValue = 'markup'; const reset = document.createElement('button'); reset.type = 'reset'; form.append(text, reset); document.body.appendChild(form);
  text.value = 'changed'; form.addEventListener('reset', (event) => event.preventDefault(), { once: true }); reset.click(); assertEquals(text.value, 'changed');
  reset.disabled = true; reset.click(); assertEquals(text.value, 'changed');
  reset.disabled = false; reset.click(); assertEquals(text.value, 'markup');
});
"#,
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn DOM reset cancellation test");
    assert!(
        ran.status.success(),
        "reset cancellation test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_input_indeterminate_is_boolean_non_reflecting_state() {
    let dir = build_dir("t_test_dom_input_indeterminate");
    write_in(
        &dir,
        "indeterminate.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('input indeterminate defaults false and remains independent of checked and attributes', () => {\n\
           const input = document.createElement('input'); input.type = 'checkbox';\n\
           assertEquals([input.indeterminate, input.hasAttribute('indeterminate')], [false, false]);\n\
           input.indeterminate = 1; input.checked = true;\n\
           assertEquals([input.indeterminate, input.checked, input.hasAttribute('indeterminate')], [true, true, false]);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom input indeterminate");
    assert!(
        ran.status.success(),
        "DOM input indeterminate test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dom_parser_accepts_well_formed_svg_foreign_content() {
    let dir = build_dir("t_test_dom_svg_foreign_content");
    write_in(
        &dir,
        "svg.test.mjs",
        "import { test, assertEquals, assertThrows } from 'runtime:test';\n\
         test('SVG attributes and self-closing descendants parse without HTML recovery', () => {\n\
           document.body.innerHTML = '<svg viewbox=\"0 0 10 10\" gradientUnits=\"userSpaceOnUse\"><circle cx=\"5\" cy=\"5\" r=\"4\"/></svg>';\n\
           const svg = document.body.firstElementChild; const circle = svg.firstElementChild;\n\
           assertEquals(svg.getAttribute('viewBox'), '0 0 10 10'); assertEquals(svg.getAttribute('gradientUnits'), 'userSpaceOnUse'); assertEquals(circle.localName, 'circle'); assertEquals(circle.getAttribute('r'), '4');\n\
           document.body.innerHTML = '<div><svg:svg><svg:circle cx=\"1\"/></svg:svg></div>';\n\
           assertEquals(document.body.firstElementChild.firstElementChild.firstElementChild.getAttribute('cx'), '1');\n\
           assertThrows(() => { document.body.innerHTML = '<div/>'; }, SyntaxError);\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom SVG foreign content");
    assert!(
        ran.status.success(),
        "DOM SVG foreign-content test did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
