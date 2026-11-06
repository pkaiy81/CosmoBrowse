// The web platform that isn't worth writing natively, built on `__cosmo`.
// Evaluated once in every page's context, before any of its scripts.
(function (global) {
  'use strict';
  const native = global.__cosmo;
  delete global.__cosmo;

  const define = (object, name, descriptor) =>
    Object.defineProperty(object, name, Object.assign({ configurable: true, enumerable: true }, descriptor));
  const hide = (object, name, value) =>
    Object.defineProperty(object, name, { value, writable: true, configurable: true, enumerable: false });
  const illegal = (name) => {
    const ctor = function () { throw new TypeError('Illegal constructor'); };
    Object.defineProperty(ctor, 'name', { value: name });
    return ctor;
  };

  global.self = global;
  global.globalThis = global;
  global.top = global;
  global.frames = global;
  global.length = 0;
  global.frameElement = null;
  global.opener = null;
  global.closed = false;
  global.name = '';

  const location = global.location;

  // ---- EventTarget -------------------------------------------------------

  class EventTarget {
    constructor() { hide(this, '__listeners', new Map()); }
    addEventListener(type, callback, options) {
      if (!callback) return;
      if (!this.__listeners) hide(this, '__listeners', new Map());
      const capture = typeof options === 'boolean' ? options : !!(options && options.capture);
      const once = !!(options && options.once);
      let list = this.__listeners.get(type);
      if (!list) this.__listeners.set(type, (list = []));
      if (!list.some((l) => l.callback === callback && l.capture === capture)) {
        list.push({ callback, capture, once });
      }
      if (options && options.signal) {
        options.signal.addEventListener('abort', () => this.removeEventListener(type, callback, options));
      }
    }
    removeEventListener(type, callback, options) {
      const capture = typeof options === 'boolean' ? options : !!(options && options.capture);
      const list = this.__listeners && this.__listeners.get(type);
      if (!list) return;
      const index = list.findIndex((l) => l.callback === callback && l.capture === capture);
      if (index >= 0) list.splice(index, 1);
    }
    dispatchEvent(event) {
      event.target = this;
      fireAt(this, event);
      return !event.defaultPrevented;
    }
  }
  function fireAt(target, event) {
    event.currentTarget = target;
    const list = target.__listeners && target.__listeners.get(event.type);
    if (list) {
      for (const listener of list.slice()) {
        if (listener.once) target.removeEventListener(event.type, listener.callback, listener.capture);
        try {
          if (typeof listener.callback === 'function') listener.callback.call(target, event);
          else if (listener.callback && listener.callback.handleEvent) listener.callback.handleEvent(event);
        } catch (error) { reportError(error); }
        if (event.__immediateStopped) break;
      }
    }
    const handler = target['on' + event.type];
    if (typeof handler === 'function') {
      try { handler.call(target, event); } catch (error) { reportError(error); }
    }
  }
  function reportError(error) {
    console.error(error && error.stack ? String(error.stack) : String(error));
  }
  global.reportError = reportError;
  global.EventTarget = EventTarget;

  // ---- Events ------------------------------------------------------------

  function Event(type, init) {
    if (!new.target) throw new TypeError("Constructor Event requires 'new'");
    const event = native.newEvent(
      String(type), !!(init && init.bubbles), !!(init && init.cancelable), new.target.prototype
    );
    event.composed = !!(init && init.composed);
    return event;
  }
  Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;
  hide(Event.prototype, 'composedPath', function () {
    const path = [];
    let node = this.target;
    while (node) { path.push(node); node = node.parentNode; }
    if (path.length) path.push(global);
    return path;
  });
  hide(Event.prototype, 'stopImmediatePropagation', function () {
    this.__immediateStopped = true;
    this.stopPropagation();
  });
  hide(Event.prototype, 'initEvent', function (type, bubbles, cancelable) {
    Object.defineProperty(this, 'type', { value: String(type), configurable: true, enumerable: true });
    Object.defineProperty(this, 'bubbles', { value: !!bubbles, configurable: true, enumerable: true });
    Object.defineProperty(this, 'cancelable', { value: !!cancelable, configurable: true, enumerable: true });
  });
  global.Event = Event;
  native.setProto('Event', Event.prototype);

  const eventClass = (name, parent, defaults) => {
    const ctor = {
      [name]: class extends parent {
        constructor(type, init) {
          super(type, init);
          for (const key of Object.keys(defaults)) {
            const value = init && init[key] !== undefined ? init[key] : defaults[key];
            Object.defineProperty(this, key, { value, writable: true, enumerable: true, configurable: true });
          }
        }
      },
    }[name];
    global[name] = ctor;
    return ctor;
  };
  const UIEvent = eventClass('UIEvent', Event, { view: null, detail: 0 });
  const MouseEvent = eventClass('MouseEvent', UIEvent, {
    screenX: 0, screenY: 0, clientX: 0, clientY: 0, pageX: 0, pageY: 0, x: 0, y: 0, offsetX: 0, offsetY: 0,
    ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, button: 0, buttons: 0, relatedTarget: null,
    movementX: 0, movementY: 0,
  });
  eventClass('PointerEvent', MouseEvent, { pointerId: 0, width: 1, height: 1, pressure: 0, pointerType: 'mouse', isPrimary: true });
  eventClass('WheelEvent', MouseEvent, { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 });
  eventClass('KeyboardEvent', UIEvent, {
    key: '', code: '', location: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false,
    repeat: false, isComposing: false, keyCode: 0, charCode: 0, which: 0,
  });
  eventClass('FocusEvent', UIEvent, { relatedTarget: null });
  eventClass('InputEvent', UIEvent, { data: null, inputType: '', isComposing: false });
  eventClass('CompositionEvent', UIEvent, { data: '' });
  eventClass('TouchEvent', UIEvent, { touches: [], targetTouches: [], changedTouches: [] });
  eventClass('CustomEvent', Event, { detail: null });
  eventClass('ErrorEvent', Event, { message: '', filename: '', lineno: 0, colno: 0, error: null });
  eventClass('MessageEvent', Event, { data: null, origin: '', lastEventId: '', source: null, ports: [] });
  eventClass('PopStateEvent', Event, { state: null });
  eventClass('HashChangeEvent', Event, { oldURL: '', newURL: '' });
  eventClass('PageTransitionEvent', Event, { persisted: false });
  eventClass('ProgressEvent', Event, { lengthComputable: false, loaded: 0, total: 0 });
  eventClass('AnimationEvent', Event, { animationName: '', elapsedTime: 0, pseudoElement: '' });
  eventClass('TransitionEvent', Event, { propertyName: '', elapsedTime: 0, pseudoElement: '' });
  eventClass('StorageEvent', Event, { key: null, oldValue: null, newValue: null, url: '', storageArea: null });
  eventClass('SubmitEvent', Event, { submitter: null });
  eventClass('DragEvent', MouseEvent, { dataTransfer: null });
  eventClass('ClipboardEvent', Event, { clipboardData: null });

  // ---- The window as an event target -------------------------------------

  const windowTarget = new EventTarget();
  global.addEventListener = (type, callback, options) => windowTarget.addEventListener(type, callback, options);
  global.removeEventListener = (type, callback, options) => windowTarget.removeEventListener(type, callback, options);
  global.dispatchEvent = (event) => {
    event.target = global;
    fireWindow(event);
    return !event.defaultPrevented;
  };
  function fireWindow(event) {
    event.currentTarget = global;
    const list = windowTarget.__listeners.get(event.type);
    if (list) {
      for (const listener of list.slice()) {
        if (listener.once) windowTarget.removeEventListener(event.type, listener.callback, listener.capture);
        try {
          if (typeof listener.callback === 'function') listener.callback.call(global, event);
          else if (listener.callback && listener.callback.handleEvent) listener.callback.handleEvent(event);
        } catch (error) { reportError(error); }
        if (event.__immediateStopped) return;
      }
    }
    const handler = global['on' + event.type];
    if (typeof handler === 'function') {
      try { handler.call(global, event); } catch (error) { reportError(error); }
    }
  }
  native.setWindowDispatcher(fireWindow);
  native.setLifecycle((phase) => {
    if (phase === 'interactive') {
      readyState = 'interactive';
    } else if (phase === 'complete') {
      readyState = 'complete';
      global.document.dispatchEvent(new Event('readystatechange'));
      global.dispatchEvent(new Event('load'));
      global.dispatchEvent(new PageTransitionEvent('pageshow'));
    } else if (phase === 'resize') {
      global.dispatchEvent(new Event('resize'));
    }
  });
  let readyState = 'loading';

  // ---- DOM classes -------------------------------------------------------

  const Node = illegal('Node');
  Node.prototype = native.nodeBase;
  Object.setPrototypeOf(Node.prototype, EventTarget.prototype);
  hide(Node.prototype, 'constructor', Node);
  Object.assign(Node, {
    ELEMENT_NODE: 1, ATTRIBUTE_NODE: 2, TEXT_NODE: 3, CDATA_SECTION_NODE: 4, PROCESSING_INSTRUCTION_NODE: 7,
    COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10, DOCUMENT_FRAGMENT_NODE: 11,
    DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2, DOCUMENT_POSITION_FOLLOWING: 4,
    DOCUMENT_POSITION_CONTAINS: 8, DOCUMENT_POSITION_CONTAINED_BY: 16,
  });
  for (const key of ['ELEMENT_NODE', 'TEXT_NODE', 'COMMENT_NODE', 'DOCUMENT_NODE', 'DOCUMENT_TYPE_NODE', 'DOCUMENT_FRAGMENT_NODE']) {
    hide(Node.prototype, key, Node[key]);
  }
  global.Node = Node;

  const subclass = (name, parent) => {
    const ctor = illegal(name);
    ctor.prototype = Object.create(parent.prototype);
    hide(ctor.prototype, 'constructor', ctor);
    Object.setPrototypeOf(ctor, parent);
    global[name] = ctor;
    native.setProto(name, ctor.prototype);
    return ctor;
  };
  const Element = subclass('Element', Node);
  const CharacterData = subclass('CharacterData', Node);
  subclass('Text', CharacterData);
  subclass('Comment', CharacterData);
  subclass('DocumentType', Node);
  const DocumentFragment = subclass('DocumentFragment', Node);
  subclass('ShadowRoot', DocumentFragment);
  const Document = subclass('Document', Node);
  subclass('HTMLDocument', Document);
  subclass('SVGElement', Element);
  subclass('MathMLElement', Element);

  // Custom elements construct through this, so `class extends HTMLElement` works.
  let constructing = null;
  const customRegistry = new Map();
  function HTMLElement() {
    if (!new.target) throw new TypeError("Constructor HTMLElement requires 'new'");
    if (constructing) {
      const element = constructing;
      constructing = null;
      Object.setPrototypeOf(element, new.target.prototype);
      return element;
    }
    for (const [name, definition] of customRegistry) {
      if (definition.ctor === new.target) {
        const element = native.createElement(name);
        Object.setPrototypeOf(element, new.target.prototype);
        return element;
      }
    }
    throw new TypeError('Illegal constructor');
  }
  HTMLElement.prototype = Object.create(Element.prototype);
  hide(HTMLElement.prototype, 'constructor', HTMLElement);
  Object.setPrototypeOf(HTMLElement, Element);
  global.HTMLElement = HTMLElement;
  native.setProto('HTMLElement', HTMLElement.prototype);

  const htmlClasses = {
    a: 'Anchor', abbr: '', area: 'Area', audio: 'Audio', base: 'Base', body: 'Body', br: 'BR', button: 'Button',
    canvas: 'Canvas', data: 'Data', datalist: 'DataList', details: 'Details', dialog: 'Dialog', div: 'Div',
    dl: 'DList', embed: 'Embed', fieldset: 'FieldSet', form: 'Form', h1: 'Heading', h2: 'Heading', h3: 'Heading',
    h4: 'Heading', h5: 'Heading', h6: 'Heading', head: 'Head', hr: 'HR', html: 'Html', iframe: 'IFrame', img: 'Image',
    input: 'Input', label: 'Label', legend: 'Legend', li: 'LI', link: 'Link', map: 'Map', menu: 'Menu', meta: 'Meta',
    meter: 'Meter', object: 'Object', ol: 'OList', optgroup: 'OptGroup', option: 'Option', output: 'Output',
    p: 'Paragraph', picture: 'Picture', pre: 'Pre', progress: 'Progress', q: 'Quote', script: 'Script',
    select: 'Select', source: 'Source', span: 'Span', style: 'Style', table: 'Table', tbody: 'TableSection',
    thead: 'TableSection', tfoot: 'TableSection', td: 'TableCell', th: 'TableCell', tr: 'TableRow',
    template: 'Template', textarea: 'TextArea', time: 'Time', title: 'Title', track: 'Track', ul: 'UList', video: 'Video',
  };
  const madeClasses = new Map();
  const HTMLMediaElement = subclass('HTMLMediaElement', HTMLElement);
  for (const tag of Object.keys(htmlClasses)) {
    if (!htmlClasses[tag]) continue;
    const name = 'HTML' + htmlClasses[tag] + 'Element';
    if (!madeClasses.has(name)) {
      madeClasses.set(name, subclass(name, tag === 'audio' || tag === 'video' ? HTMLMediaElement : HTMLElement));
    }
    native.setProto(tag, madeClasses.get(name).prototype);
  }

  // ---- Node and Element, on top of the native base -----------------------

  const get = (proto, name, getter, setter) => define(proto, name, { get: getter, set: setter });
  const childElements = function* (node) {
    let child = node.firstChild;
    while (child) { if (child.nodeType === 1) yield child; child = child.nextSibling; }
  };
  const convertNodes = (nodes) => nodes.map((n) => (n instanceof Node ? n : global.document.createTextNode(String(n))));

  get(Node.prototype, 'isConnected', function () {
    let node = this;
    while (node) { if (node.nodeType === 9) return true; node = node.parentNode; }
    return false;
  });
  get(Node.prototype, 'ownerDocument', function () { return this.nodeType === 9 ? null : global.document; });
  hide(Node.prototype, 'getRootNode', function () {
    let node = this;
    while (node.parentNode) node = node.parentNode;
    return node;
  });
  hide(Node.prototype, 'hasChildNodes', function () { return this.firstChild !== null; });
  hide(Node.prototype, 'contains', function (other) {
    while (other) { if (other === this) return true; other = other.parentNode; }
    return false;
  });
  hide(Node.prototype, 'normalize', function () {});
  hide(Node.prototype, 'isSameNode', function (other) { return this === other; });
  hide(Node.prototype, 'isEqualNode', function (other) { return !!other && this.outerHTML === other.outerHTML; });
  hide(Node.prototype, 'replaceChild', function (fresh, old) {
    this.insertBefore(fresh, old);
    this.removeChild(old);
    return old;
  });
  hide(Node.prototype, 'compareDocumentPosition', function (other) {
    if (this === other) return 0;
    if (this.contains(other)) return 16 | 4;
    if (other.contains(this)) return 8 | 2;
    if (this.getRootNode() !== other.getRootNode()) return 1 | 32;
    const path = (node) => { const p = []; while (node) { p.unshift(node); node = node.parentNode; } return p; };
    const a = path(this), b = path(other);
    let i = 0;
    while (a[i] === b[i]) i++;
    let sibling = a[i];
    while (sibling) { if (sibling === b[i]) return 4; sibling = sibling.nextSibling; }
    return 2;
  });
  get(Node.prototype, 'nodeValue',
    function () { return this.nodeType === 3 || this.nodeType === 8 ? this.textContent : null; },
    function (value) { if (this.nodeType === 3 || this.nodeType === 8) this.textContent = value; });
  get(CharacterData.prototype, 'data', function () { return this.textContent; }, function (value) { this.textContent = value; });
  get(CharacterData.prototype, 'length', function () { return this.textContent.length; });
  get(global.Text.prototype, 'wholeText', function () { return this.textContent; });

  const parentNodeMixin = (proto) => {
    get(proto, 'firstElementChild', function () { return childElements(this).next().value || null; });
    get(proto, 'lastElementChild', function () {
      let child = this.lastChild;
      while (child && child.nodeType !== 1) child = child.previousSibling;
      return child || null;
    });
    get(proto, 'childElementCount', function () { let n = 0; for (const _ of childElements(this)) n++; return n; });
    hide(proto, 'append', function (...nodes) { for (const n of convertNodes(nodes)) this.appendChild(n); });
    hide(proto, 'prepend', function (...nodes) {
      const first = this.firstChild;
      for (const n of convertNodes(nodes)) this.insertBefore(n, first);
    });
    hide(proto, 'replaceChildren', function (...nodes) {
      while (this.firstChild) this.removeChild(this.firstChild);
      for (const n of convertNodes(nodes)) this.appendChild(n);
    });
  };
  parentNodeMixin(Element.prototype);
  parentNodeMixin(Document.prototype);
  parentNodeMixin(DocumentFragment.prototype);

  const childNodeMixin = (proto) => {
    hide(proto, 'before', function (...nodes) {
      const parent = this.parentNode;
      if (parent) for (const n of convertNodes(nodes)) parent.insertBefore(n, this);
    });
    hide(proto, 'after', function (...nodes) {
      const parent = this.parentNode;
      if (!parent) return;
      const next = this.nextSibling;
      for (const n of convertNodes(nodes)) parent.insertBefore(n, next);
    });
    hide(proto, 'replaceWith', function (...nodes) {
      const parent = this.parentNode;
      if (!parent) return;
      for (const n of convertNodes(nodes)) parent.insertBefore(n, this);
      parent.removeChild(this);
    });
    get(proto, 'nextElementSibling', function () {
      let node = this.nextSibling;
      while (node && node.nodeType !== 1) node = node.nextSibling;
      return node || null;
    });
    get(proto, 'previousElementSibling', function () {
      let node = this.previousSibling;
      while (node && node.nodeType !== 1) node = node.previousSibling;
      return node || null;
    });
  };
  childNodeMixin(Element.prototype);
  childNodeMixin(CharacterData.prototype);

  get(Element.prototype, 'localName', function () { return this.tagName.toLowerCase(); });
  get(Element.prototype, 'namespaceURI', function () {
    return native.namespaceOf(this);
  });
  get(Element.prototype, 'prefix', function () { return null; });
  get(Element.prototype, 'slot', function () { return this.getAttribute('slot') || ''; }, function (v) { this.setAttribute('slot', v); });
  hide(Element.prototype, 'getAttributeNames', function () { return native.attributes(this).map((a) => a[0]); });
  hide(Element.prototype, 'hasAttributes', function () { return native.attributes(this).length > 0; });
  hide(Element.prototype, 'toggleAttribute', function (name, force) {
    const has = this.hasAttribute(name);
    const want = force === undefined ? !has : !!force;
    if (want && !has) this.setAttribute(name, '');
    if (!want && has) this.removeAttribute(name);
    return want;
  });
  hide(Element.prototype, 'getAttributeNS', function (_, name) { return this.getAttribute(name); });
  hide(Element.prototype, 'setAttributeNS', function (_, name, value) { this.setAttribute(name, value); });
  hide(Element.prototype, 'removeAttributeNS', function (_, name) { this.removeAttribute(name); });
  hide(Element.prototype, 'hasAttributeNS', function (_, name) { return this.hasAttribute(name); });
  get(Element.prototype, 'attributes', function () {
    const element = this;
    const list = native.attributes(element).map((a) => ({
      name: a[0], localName: a[0], nodeName: a[0], value: a[1], nodeValue: a[1], ownerElement: element, specified: true,
    }));
    list.getNamedItem = (name) => list.find((a) => a.name === name) || null;
    list.item = (i) => list[i] || null;
    return list;
  });
  get(Element.prototype, 'outerHTML', function () { return native.outerHTML(this); }, function (html) {
    const parent = this.parentNode;
    if (!parent) return;
    const context = parent.nodeType === 1 ? parent.localName : 'body';
    for (const node of native.parseFragment(context, String(html))) parent.insertBefore(node, this);
    parent.removeChild(this);
  });
  hide(Element.prototype, 'insertAdjacentHTML', function (position, html) {
    const where = String(position).toLowerCase();
    const inside = where === 'afterbegin' || where === 'beforeend';
    const parent = inside ? this : this.parentNode;
    if (!parent) return;
    const context = parent.nodeType === 1 ? parent.localName : 'body';
    const reference = where === 'afterbegin' ? this.firstChild
      : where === 'beforebegin' ? this
      : where === 'afterend' ? this.nextSibling : null;
    for (const node of native.parseFragment(context, String(html))) parent.insertBefore(node, reference);
  });
  hide(Element.prototype, 'insertAdjacentElement', function (position, element) {
    const where = String(position).toLowerCase();
    if (where === 'beforebegin') this.parentNode && this.parentNode.insertBefore(element, this);
    else if (where === 'afterbegin') this.insertBefore(element, this.firstChild);
    else if (where === 'beforeend') this.appendChild(element);
    else if (where === 'afterend') this.parentNode && this.parentNode.insertBefore(element, this.nextSibling);
    return element;
  });
  hide(Element.prototype, 'insertAdjacentText', function (position, text) {
    this.insertAdjacentElement(position, global.document.createTextNode(String(text)));
  });

  // Geometry comes from layout.
  const rectOf = (element) => native.rectOf(element) || [0, 0, 0, 0];
  function DOMRect(x = 0, y = 0, width = 0, height = 0) {
    Object.assign(this, { x, y, width, height, left: x, top: y, right: x + width, bottom: y + height });
  }
  DOMRect.prototype.toJSON = function () { return Object.assign({}, this); };
  global.DOMRect = DOMRect;
  global.DOMRectReadOnly = DOMRect;
  hide(Element.prototype, 'getBoundingClientRect', function () {
    const [x, y, w, h] = rectOf(this);
    return new DOMRect(x - global.scrollX, y - global.scrollY, w, h);
  });
  hide(Element.prototype, 'getClientRects', function () {
    const rect = this.getBoundingClientRect();
    const list = rect.width || rect.height ? [rect] : [];
    list.item = (i) => list[i] || null;
    return list;
  });
  get(HTMLElement.prototype, 'offsetWidth', function () { return rectOf(this)[2]; });
  get(HTMLElement.prototype, 'offsetHeight', function () { return rectOf(this)[3]; });
  get(HTMLElement.prototype, 'offsetLeft', function () { return rectOf(this)[0]; });
  get(HTMLElement.prototype, 'offsetTop', function () { return rectOf(this)[1]; });
  get(HTMLElement.prototype, 'offsetParent', function () { return this.parentElement; });
  get(Element.prototype, 'clientWidth', function () {
    return this === global.document.documentElement ? global.innerWidth : rectOf(this)[2];
  });
  get(Element.prototype, 'clientHeight', function () {
    return this === global.document.documentElement ? global.innerHeight : rectOf(this)[3];
  });
  get(Element.prototype, 'clientLeft', function () { return 0; });
  get(Element.prototype, 'clientTop', function () { return 0; });
  get(Element.prototype, 'scrollWidth', function () { return rectOf(this)[2]; });
  get(Element.prototype, 'scrollHeight', function () { return rectOf(this)[3]; });
  const scrollPositions = new WeakMap();
  const scrollProperty = (name, index) => get(Element.prototype, name,
    function () { return (scrollPositions.get(this) || [0, 0])[index]; },
    function (value) {
      const position = scrollPositions.get(this) || [0, 0];
      position[index] = Number(value) || 0;
      scrollPositions.set(this, position);
    });
  scrollProperty('scrollLeft', 0);
  scrollProperty('scrollTop', 1);
  hide(Element.prototype, 'scrollIntoView', function () {});
  hide(Element.prototype, 'scrollTo', function () {});
  hide(Element.prototype, 'scrollBy', function () {});
  hide(Element.prototype, 'scroll', function () {});
  hide(Element.prototype, 'animate', function () {
    const finished = Promise.resolve();
    return { finished, ready: finished, cancel() {}, finish() {}, pause() {}, play() {}, reverse() {}, addEventListener() {}, removeEventListener() {}, onfinish: null, playState: 'finished' };
  });
  hide(Element.prototype, 'getAnimations', function () { return []; });
  hide(Element.prototype, 'requestFullscreen', function () { return Promise.reject(new TypeError('not supported')); });
  hide(Element.prototype, 'setPointerCapture', function () {});
  hide(Element.prototype, 'releasePointerCapture', function () {});
  hide(Element.prototype, 'hasPointerCapture', function () { return false; });
  hide(Element.prototype, 'getElementsByTagName', function (tag) {
    return Array.from(this.querySelectorAll(tag));
  });
  hide(Element.prototype, 'getElementsByClassName', function (names) {
    return Array.from(this.querySelectorAll(String(names).trim().split(/\s+/).map((n) => '.' + n).join('')));
  });

  // Shadow DOM isn't there: a shadow root is a box inside the element, without
  // the encapsulation, so components at least render.
  hide(Element.prototype, 'attachShadow', function (init) {
    const root = native.createElement('cosmo-shadow-root');
    root.setAttribute('style', 'display:contents');
    Object.setPrototypeOf(root, global.ShadowRoot.prototype);
    Object.defineProperty(root, 'host', { value: this, enumerable: true });
    Object.defineProperty(root, 'mode', { value: (init && init.mode) || 'open', enumerable: true });
    get(root, 'innerHTML',
      function () { return native.innerHTMLOf(root); },
      function (html) { native.setInnerHTML(root, html); });
    this.insertBefore(root, this.firstChild);
    Object.defineProperty(this, 'shadowRoot', { value: init && init.mode === 'closed' ? null : root, configurable: true });
    return root;
  });
  get(Element.prototype, 'shadowRoot', function () { return null; });
  get(global.ShadowRoot.prototype, 'activeElement', function () { return null; });
  for (const name of ['querySelector', 'querySelectorAll', 'getElementById']) {
    if (Node.prototype[name]) hide(global.ShadowRoot.prototype, name, Node.prototype[name]);
  }
  hide(global.ShadowRoot.prototype, 'getElementById', function (id) { return this.querySelector('#' + id); });

  // HTMLElement.
  const reflect = (proto, property, attribute = property.toLowerCase()) => get(proto, property,
    function () { const v = this.getAttribute(attribute); return v === null ? '' : v; },
    function (value) { this.setAttribute(attribute, String(value)); });
  const reflectBool = (proto, property, attribute = property.toLowerCase()) => get(proto, property,
    function () { return this.hasAttribute(attribute); },
    function (value) { if (value) this.setAttribute(attribute, ''); else this.removeAttribute(attribute); });
  const reflectNumber = (proto, property, fallback, attribute = property.toLowerCase()) => get(proto, property,
    function () { const v = parseInt(this.getAttribute(attribute), 10); return Number.isNaN(v) ? fallback : v; },
    function (value) { this.setAttribute(attribute, String(value)); });
  const reflectUrl = (proto, property, attribute = property.toLowerCase()) => get(proto, property,
    function () {
      const v = this.getAttribute(attribute);
      if (v === null) return '';
      const parsed = native.parseUrl(v, global.location.href);
      return parsed ? parsed.href : v;
    },
    function (value) { this.setAttribute(attribute, String(value)); });
  for (const name of ['title', 'lang', 'dir', 'accessKey', 'translate']) reflect(HTMLElement.prototype, name);
  reflectBool(HTMLElement.prototype, 'hidden');
  reflectBool(HTMLElement.prototype, 'inert');
  reflectNumber(HTMLElement.prototype, 'tabIndex', -1, 'tabindex');
  get(HTMLElement.prototype, 'isContentEditable', function () { return this.getAttribute('contenteditable') === 'true'; });
  get(HTMLElement.prototype, 'contentEditable',
    function () { return this.getAttribute('contenteditable') || 'inherit'; },
    function (v) { this.setAttribute('contenteditable', v); });
  get(HTMLElement.prototype, 'draggable', function () { return this.getAttribute('draggable') === 'true'; },
    function (v) { this.setAttribute('draggable', String(!!v)); });
  get(HTMLElement.prototype, 'innerText', function () { return this.textContent; }, function (v) { this.textContent = v; });
  get(HTMLElement.prototype, 'outerText', function () { return this.textContent; });
  let activeElement = null;
  hide(HTMLElement.prototype, 'focus', function () {
    if (activeElement === this) return;
    const previous = activeElement;
    activeElement = this;
    if (previous) previous.dispatchEvent(new FocusEvent('blur'));
    this.dispatchEvent(new FocusEvent('focus'));
    this.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
  });
  hide(HTMLElement.prototype, 'blur', function () {
    if (activeElement !== this) return;
    activeElement = null;
    this.dispatchEvent(new FocusEvent('blur'));
    this.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
  });
  hide(HTMLElement.prototype, 'click', function () {
    this.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  });
  const handlers = ['abort', 'animationend', 'animationiteration', 'animationstart', 'auxclick', 'beforeinput', 'blur', 'cancel',
    'change', 'click', 'close', 'contextmenu', 'copy', 'cut', 'dblclick', 'drag', 'dragend', 'dragenter', 'dragleave', 'dragover',
    'dragstart', 'drop', 'error', 'focus', 'focusin', 'focusout', 'input', 'invalid', 'keydown', 'keypress', 'keyup', 'load',
    'mousedown', 'mouseenter', 'mouseleave', 'mousemove', 'mouseout', 'mouseover', 'mouseup', 'paste', 'pointercancel',
    'pointerdown', 'pointerenter', 'pointerleave', 'pointermove', 'pointerout', 'pointerover', 'pointerup', 'reset', 'resize',
    'scroll', 'select', 'submit', 'toggle', 'touchcancel', 'touchend', 'touchmove', 'touchstart', 'transitionend', 'wheel'];
  const handlerSlots = new WeakMap();
  for (const type of handlers) {
    const property = 'on' + type;
    const accessors = {
      get() { const slots = handlerSlots.get(this); return (slots && slots[type]) || null; },
      set(value) {
        let slots = handlerSlots.get(this);
        if (!slots) handlerSlots.set(this, (slots = {}));
        if (slots[type]) this.removeEventListener(type, slots['$' + type]);
        slots[type] = typeof value === 'function' ? value : null;
        if (slots[type]) {
          const element = this;
          slots['$' + type] = function (event) {
            const result = slots[type].call(element, event);
            if (result === false) event.preventDefault();
          };
          this.addEventListener(type, slots['$' + type]);
        }
      },
      configurable: true,
      enumerable: true,
    };
    Object.defineProperty(HTMLElement.prototype, property, accessors);
    Object.defineProperty(global.SVGElement.prototype, property, accessors);
    Object.defineProperty(Document.prototype, property, accessors);
  }

  // Forms.
  const proto = (name) => global['HTML' + name + 'Element'].prototype;
  const Input = proto('Input'), TextArea = proto('TextArea'), Select = proto('Select'), Button = proto('Button');
  const Option = proto('Option'), Form = proto('Form');
  get(Input, 'value',
    function () {
      const type = (this.getAttribute('type') || '').toLowerCase();
      const value = this.getAttribute('value');
      if (value === null) return type === 'checkbox' || type === 'radio' ? 'on' : '';
      return value;
    },
    function (value) { this.setAttribute('value', value === null ? '' : String(value)); });
  reflect(Input, 'defaultValue', 'value');
  get(Input, 'valueAsNumber', function () { const n = parseFloat(this.value); return n; },
    function (v) { this.value = String(v); });
  reflectBool(Input, 'checked');
  reflectBool(Input, 'defaultChecked', 'checked');
  get(Input, 'indeterminate', function () { return false; }, function () {});
  get(Input, 'type', function () {
    return (this.getAttribute('type') || 'text').toLowerCase();
  }, function (v) { this.setAttribute('type', v); });
  for (const name of ['name', 'placeholder', 'min', 'max', 'step', 'pattern', 'accept', 'alt', 'autocomplete', 'inputMode']) reflect(Input, name);
  reflectUrl(Input, 'src');
  for (const name of ['disabled', 'required', 'readOnly', 'multiple', 'autofocus']) reflectBool(Input, name);
  reflectNumber(Input, 'maxLength', -1, 'maxlength');
  reflectNumber(Input, 'minLength', -1, 'minlength');
  reflectNumber(Input, 'size', 20);
  get(Input, 'files', function () { return []; });
  hide(Input, 'select', function () {});
  hide(Input, 'setSelectionRange', function () {});
  get(Input, 'selectionStart', function () { return this.value.length; }, function () {});
  get(Input, 'selectionEnd', function () { return this.value.length; }, function () {});

  get(TextArea, 'value', function () { return this.textContent; }, function (v) { this.textContent = String(v); });
  get(TextArea, 'defaultValue', function () { return this.textContent; }, function (v) { this.textContent = String(v); });
  get(TextArea, 'type', function () { return 'textarea'; });
  for (const name of ['name', 'placeholder', 'wrap']) reflect(TextArea, name);
  for (const name of ['disabled', 'required', 'readOnly', 'autofocus']) reflectBool(TextArea, name);
  reflectNumber(TextArea, 'rows', 2);
  reflectNumber(TextArea, 'cols', 20);
  reflectNumber(TextArea, 'maxLength', -1, 'maxlength');
  hide(TextArea, 'select', function () {});
  hide(TextArea, 'setSelectionRange', function () {});
  get(TextArea, 'selectionStart', function () { return this.value.length; }, function () {});
  get(TextArea, 'selectionEnd', function () { return this.value.length; }, function () {});

  get(Option, 'value',
    function () { const v = this.getAttribute('value'); return v === null ? this.text : v; },
    function (v) { this.setAttribute('value', String(v)); });
  get(Option, 'text', function () { return this.textContent.trim().replace(/\s+/g, ' '); }, function (v) { this.textContent = v; });
  reflect(Option, 'label');
  reflectBool(Option, 'disabled');
  reflectBool(Option, 'defaultSelected', 'selected');
  get(Option, 'selected',
    function () {
      const select = this.closest('select');
      return select ? select.selectedOptions.includes(this) : this.hasAttribute('selected');
    },
    function (value) {
      const select = this.closest('select');
      if (value && select && !select.multiple) {
        for (const option of select.options) option.removeAttribute('selected');
      }
      if (value) this.setAttribute('selected', ''); else this.removeAttribute('selected');
    });
  get(Option, 'index', function () {
    const select = this.closest('select');
    return select ? select.options.indexOf(this) : 0;
  });
  get(Option, 'form', function () { return this.closest('form'); });
  get(Select, 'options', function () {
    const list = Array.from(this.querySelectorAll('option'));
    list.item = (i) => list[i] || null;
    list.namedItem = (name) => list.find((o) => o.id === name || o.getAttribute('name') === name) || null;
    return list;
  });
  get(Select, 'length', function () { return this.options.length; });
  get(Select, 'selectedOptions', function () {
    const options = this.options;
    const marked = options.filter((o) => o.hasAttribute('selected'));
    if (this.multiple) return marked;
    if (marked.length) return [marked[marked.length - 1]];
    return options.length ? [options[0]] : [];
  });
  get(Select, 'selectedIndex',
    function () { const s = this.selectedOptions; return s.length ? this.options.indexOf(s[0]) : -1; },
    function (index) {
      const options = this.options;
      for (const option of options) option.removeAttribute('selected');
      if (options[index]) options[index].setAttribute('selected', '');
    });
  get(Select, 'value',
    function () { const s = this.selectedOptions; return s.length ? s[0].value : ''; },
    function (value) {
      for (const option of this.options) {
        if (option.value === String(value)) option.setAttribute('selected', '');
        else option.removeAttribute('selected');
      }
    });
  get(Select, 'type', function () { return this.multiple ? 'select-multiple' : 'select-one'; });
  reflect(Select, 'name');
  for (const name of ['disabled', 'required', 'multiple']) reflectBool(Select, name);
  reflectNumber(Select, 'size', 0);
  hide(Select, 'item', function (i) { return this.options[i] || null; });
  hide(Select, 'add', function (option, before) {
    const reference = typeof before === 'number' ? this.options[before] : before;
    this.insertBefore(option, reference || null);
  });

  get(Button, 'type',
    function () { const t = (this.getAttribute('type') || '').toLowerCase(); return t === 'button' || t === 'reset' ? t : 'submit'; },
    function (v) { this.setAttribute('type', v); });
  for (const name of ['name', 'value']) reflect(Button, name);
  reflectBool(Button, 'disabled');
  for (const P of [Input, TextArea, Select, Button]) {
    get(P, 'form', function () {
      const id = this.getAttribute('form');
      return id ? global.document.getElementById(id) : this.closest('form');
    });
    get(P, 'labels', function () {
      const list = [];
      if (this.id) list.push(...global.document.querySelectorAll('label[for="' + this.id + '"]'));
      const parent = this.closest('label');
      if (parent) list.push(parent);
      return list;
    });
    get(P, 'validity', function () {
      return { valid: true, valueMissing: false, typeMismatch: false, patternMismatch: false, customError: false };
    });
    get(P, 'validationMessage', function () { return ''; });
    get(P, 'willValidate', function () { return true; });
    hide(P, 'checkValidity', function () { return true; });
    hide(P, 'reportValidity', function () { return true; });
    hide(P, 'setCustomValidity', function () {});
  }
  get(Form, 'elements', function () {
    const list = Array.from(this.querySelectorAll('input,select,textarea,button,fieldset,object,output'));
    list.item = (i) => list[i] || null;
    list.namedItem = (name) => list.find((e) => e.id === name || e.getAttribute('name') === name) || null;
    return list;
  });
  get(Form, 'length', function () { return this.elements.length; });
  for (const name of ['name', 'method', 'target', 'enctype', 'acceptCharset']) reflect(Form, name);
  reflectUrl(Form, 'action');
  reflectBool(Form, 'noValidate', 'novalidate');
  const submitForm = (form, submitter) => {
    const pick = (submitterAttribute, formAttribute) =>
      (submitter && submitter.getAttribute(submitterAttribute)) || form.getAttribute(formAttribute) || '';
    const url = new URL(pick('formaction', 'action') || location.href, location.href);
    url.hash = '';
    const params = new URLSearchParams();
    for (const [name, value] of native.formData(form)) params.append(name, value);
    if (submitter && submitter.getAttribute('name')) params.append(submitter.getAttribute('name'), submitter.getAttribute('value') || '');
    if (pick('formmethod', 'method').toLowerCase() === 'post') {
      native.navigate(url.href, params.toString());
    } else {
      url.search = params.toString();
      native.navigate(url.href);
    }
  };
  hide(Form, 'submit', function () { submitForm(this, null); });
  hide(Form, 'requestSubmit', function (submitter) {
    if (this.dispatchEvent(new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: submitter || null }))) {
      submitForm(this, submitter || null);
    }
  });
  hide(Form, 'reset', function () {
    if (this.dispatchEvent(new Event('reset', { bubbles: true, cancelable: true }))) {
      for (const element of this.elements) {
        if (element.localName === 'textarea' || element.localName === 'select') continue;
      }
    }
  });
  hide(Form, 'checkValidity', function () { return true; });
  hide(Form, 'reportValidity', function () { return true; });

  // Links and embedded content.
  const urlParts = ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password'];
  for (const P of [proto('Anchor'), proto('Area')]) {
    reflectUrl(P, 'href');
    for (const name of ['target', 'rel', 'download', 'hreflang', 'type', 'referrerPolicy']) reflect(P, name);
    for (const part of urlParts) {
      get(P, part, function () {
        const parsed = native.parseUrl(this.getAttribute('href') || '', global.location.href);
        return parsed ? parsed[part] : '';
      });
    }
    hide(P, 'toString', function () { return this.href; });
    get(P, 'relList', function () { return tokenListFor(this, 'rel'); });
  }
  get(proto('Anchor'), 'text', function () { return this.textContent; }, function (v) { this.textContent = v; });
  const Image_ = proto('Image');
  reflectUrl(Image_, 'src');
  for (const name of ['alt', 'srcset', 'sizes', 'crossOrigin', 'loading', 'decoding', 'referrerPolicy', 'useMap']) reflect(Image_, name);
  reflectNumber(Image_, 'width', 0);
  reflectNumber(Image_, 'height', 0);
  get(Image_, 'naturalWidth', function () { return rectOf(this)[2]; });
  get(Image_, 'naturalHeight', function () { return rectOf(this)[3]; });
  get(Image_, 'complete', function () { return true; });
  get(Image_, 'currentSrc', function () { return this.src; });
  hide(Image_, 'decode', function () { return Promise.resolve(); });
  global.Image = function (width, height) {
    const image = global.document.createElement('img');
    if (width !== undefined) image.width = width;
    if (height !== undefined) image.height = height;
    return image;
  };
  global.Option = function (text, value, defaultSelected, selected) {
    const option = global.document.createElement('option');
    if (text !== undefined) option.textContent = text;
    if (value !== undefined) option.value = value;
    if (defaultSelected || selected) option.setAttribute('selected', '');
    return option;
  };
  const Link = proto('Link');
  reflectUrl(Link, 'href');
  for (const name of ['rel', 'media', 'type', 'as', 'crossOrigin', 'integrity', 'hreflang', 'sizes']) reflect(Link, name);
  get(Link, 'relList', function () { return tokenListFor(this, 'rel'); });
  get(Link, 'sheet', function () { return null; });
  const Script = proto('Script');
  reflectUrl(Script, 'src');
  for (const name of ['type', 'charset', 'crossOrigin', 'integrity', 'referrerPolicy']) reflect(Script, name);
  for (const name of ['async', 'defer', 'noModule']) reflectBool(Script, name);
  get(Script, 'text', function () { return this.textContent; }, function (v) { this.textContent = v; });
  const Style = proto('Style');
  reflect(Style, 'media');
  reflect(Style, 'type');
  get(Style, 'sheet', function () { return makeStyleSheet(this); });
  get(proto('Meta'), 'content', function () { return this.getAttribute('content') || ''; }, function (v) { this.setAttribute('content', v); });
  reflect(proto('Meta'), 'name');
  reflect(proto('Meta'), 'httpEquiv', 'http-equiv');
  reflectUrl(proto('Base'), 'href');
  reflect(proto('Base'), 'target');
  reflectBool(proto('Details'), 'open');
  const Dialog = proto('Dialog');
  reflectBool(Dialog, 'open');
  hide(Dialog, 'show', function () { this.setAttribute('open', ''); });
  hide(Dialog, 'showModal', function () { this.setAttribute('open', ''); });
  hide(Dialog, 'close', function (value) {
    this.removeAttribute('open');
    if (value !== undefined) this.returnValue = value;
    this.dispatchEvent(new Event('close'));
  });
  const Label = proto('Label');
  reflect(Label, 'htmlFor', 'for');
  get(Label, 'control', function () {
    const id = this.getAttribute('for');
    return id ? global.document.getElementById(id) : this.querySelector('input,select,textarea,button');
  });
  const Media = HTMLMediaElement.prototype;
  reflectUrl(Media, 'src');
  for (const name of ['autoplay', 'controls', 'loop', 'muted']) reflectBool(Media, name);
  hide(Media, 'play', function () { return Promise.reject(new DOMException('Playback is not supported', 'NotSupportedError')); });
  hide(Media, 'pause', function () {});
  hide(Media, 'load', function () {});
  hide(Media, 'canPlayType', function () { return ''; });
  get(Media, 'paused', function () { return true; });
  get(Media, 'duration', function () { return NaN; });
  get(Media, 'currentTime', function () { return 0; }, function () {});
  get(Media, 'volume', function () { return 1; }, function () {});
  get(Media, 'readyState', function () { return 0; });
  const Canvas = proto('Canvas');
  reflectNumber(Canvas, 'width', 300);
  reflectNumber(Canvas, 'height', 150);
  hide(Canvas, 'getContext', function () { return null; });
  hide(Canvas, 'toDataURL', function () { return 'data:,'; });
  const IFrame = proto('IFrame');
  reflectUrl(IFrame, 'src');
  for (const name of ['srcdoc', 'name', 'sandbox', 'allow', 'loading', 'width', 'height']) reflect(IFrame, name);
  get(IFrame, 'contentWindow', function () { return null; });
  get(IFrame, 'contentDocument', function () { return null; });
  const Table = proto('Table');
  get(Table, 'rows', function () { return Array.from(this.querySelectorAll('tr')); });
  get(Table, 'tBodies', function () { return Array.from(this.querySelectorAll('tbody')); });
  get(Table, 'tHead', function () { return this.querySelector('thead'); });
  get(Table, 'tFoot', function () { return this.querySelector('tfoot'); });
  get(Table, 'caption', function () { return this.querySelector('caption'); });
  get(proto('TableSection'), 'rows', function () { return Array.from(this.querySelectorAll('tr')); });
  get(proto('TableRow'), 'cells', function () { return Array.from(this.children); });
  get(proto('TableRow'), 'rowIndex', function () {
    const table = this.closest('table');
    return table ? table.rows.indexOf(this) : -1;
  });
  get(proto('TableCell'), 'cellIndex', function () { return this.parentNode ? Array.from(this.parentNode.children).indexOf(this) : -1; });
  reflectNumber(proto('TableCell'), 'colSpan', 1, 'colspan');
  reflectNumber(proto('TableCell'), 'rowSpan', 1, 'rowspan');
  get(proto('Template'), 'content', function () { return this; });
  reflectNumber(proto('LI'), 'value', 0);
  reflectNumber(proto('OList'), 'start', 1);
  reflect(proto('Time'), 'dateTime', 'datetime');
  reflect(proto('Data'), 'value');
  const Progress = proto('Progress');
  reflectNumber(Progress, 'value', 0);
  reflectNumber(Progress, 'max', 1);
  const Meter = proto('Meter');
  reflectNumber(Meter, 'value', 0);
  reflectNumber(Meter, 'min', 0);
  reflectNumber(Meter, 'max', 1);

  function tokenListFor(element, attribute) {
    const read = () => (element.getAttribute(attribute) || '').split(/\s+/).filter(Boolean);
    return {
      contains: (t) => read().includes(t),
      add: (...tokens) => element.setAttribute(attribute, [...new Set([...read(), ...tokens])].join(' ')),
      remove: (...tokens) => element.setAttribute(attribute, read().filter((t) => !tokens.includes(t)).join(' ')),
      supports: () => true,
      get length() { return read().length; },
      get value() { return read().join(' '); },
    };
  }

  // ---- Document ----------------------------------------------------------

  const documentProto = global.HTMLDocument.prototype;
  for (const name of ['getElementById', 'createTextNode', 'getElementsByTagName', 'getElementsByClassName']) {
    hide(documentProto, name, native.documentMethods[name]);
  }
  hide(documentProto, 'querySelector', Node.prototype.querySelector);
  hide(documentProto, 'querySelectorAll', Node.prototype.querySelectorAll);
  hide(documentProto, 'createElement', function (tag) {
    const name = String(tag).toLowerCase();
    const element = native.createElement(name);
    const definition = customRegistry.get(name);
    if (definition) upgrade(element, definition);
    return element;
  });
  hide(documentProto, 'createElementNS', function (namespace, name) { return native.createElementNS(namespace, String(name)); });
  hide(documentProto, 'createComment', function (text) { return native.createComment(String(text)); });
  hide(documentProto, 'createDocumentFragment', function () {
    const fragment = native.createElement('cosmo-fragment');
    Object.setPrototypeOf(fragment, DocumentFragment.prototype);
    return fragment;
  });
  hide(DocumentFragment.prototype, 'getElementById', function (id) { return this.querySelector('#' + id); });
  hide(documentProto, 'createEvent', function (kind) {
    const ctor = global[String(kind).replace(/s$/, '')] || Event;
    return new ctor('');
  });
  hide(documentProto, 'createRange', function () {
    const range = {
      setStart() {}, setEnd() {}, setStartBefore() {}, setEndAfter() {}, selectNode() {}, selectNodeContents() {},
      collapse() {}, cloneRange() { return range; }, detach() {}, getBoundingClientRect() { return new DOMRect(); },
      getClientRects() { return []; }, collapsed: true, commonAncestorContainer: document,
      createContextualFragment(html) {
        const fragment = document.createDocumentFragment();
        for (const node of native.parseFragment('body', String(html))) fragment.appendChild(node);
        return fragment;
      },
    };
    return range;
  });
  hide(documentProto, 'createTreeWalker', function (root) {
    const list = [];
    const collect = (node) => { for (let c = node.firstChild; c; c = c.nextSibling) { list.push(c); collect(c); } };
    collect(root);
    let index = -1;
    return { root, currentNode: root, nextNode() { index++; this.currentNode = list[index] || null; return this.currentNode; } };
  });
  hide(documentProto, 'importNode', function (node, deep) { return node.cloneNode(!!deep); });
  hide(documentProto, 'adoptNode', function (node) { node.remove && node.remove(); return node; });
  hide(documentProto, 'hasFocus', function () { return true; });
  hide(documentProto, 'execCommand', function () { return false; });
  hide(documentProto, 'queryCommandSupported', function () { return false; });
  hide(documentProto, 'elementFromPoint', function () { return this.body; });
  hide(documentProto, 'elementsFromPoint', function () { return [this.body]; });
  hide(documentProto, 'getSelection', function () { return global.getSelection(); });
  hide(documentProto, 'open', function () { return this; });
  hide(documentProto, 'close', function () {});
  hide(documentProto, 'write', function (...text) {
    const html = text.join('');
    const script = this.currentScript;
    const parent = script ? script.parentNode : this.body;
    if (!parent) return;
    const context = parent.nodeType === 1 ? parent.localName : 'body';
    const reference = script ? script.nextSibling : null;
    for (const node of native.parseFragment(context, html)) parent.insertBefore(node, reference);
  });
  hide(documentProto, 'writeln', function (...text) { this.write(text.join('') + '\n'); });
  const elementChild = (tag) => function () {
    const root = this.documentElement;
    if (!root) return null;
    for (const child of childElements(root)) if (child.localName === tag) return child;
    return null;
  };
  get(documentProto, 'documentElement', function () { return childElements(this).next().value || null; });
  get(documentProto, 'head', elementChild('head'));
  get(documentProto, 'body', function () {
    const root = this.documentElement;
    if (!root) return null;
    for (const child of childElements(root)) if (child.localName === 'body' || child.localName === 'frameset') return child;
    return null;
  });
  get(documentProto, 'title',
    function () { const t = this.querySelector('title'); return t ? t.textContent.trim().replace(/\s+/g, ' ') : ''; },
    function (value) {
      let t = this.querySelector('title');
      if (!t && this.head) t = this.head.appendChild(this.createElement('title'));
      if (t) t.textContent = value;
    });
  const collection = (selector) => function () { return Array.from(this.querySelectorAll(selector)); };
  get(documentProto, 'forms', collection('form'));
  get(documentProto, 'images', collection('img'));
  get(documentProto, 'links', collection('a[href],area[href]'));
  get(documentProto, 'scripts', collection('script'));
  get(documentProto, 'embeds', collection('embed'));
  get(documentProto, 'styleSheets', function () { return Array.from(this.querySelectorAll('style')).map(makeStyleSheet); });
  get(documentProto, 'adoptedStyleSheets', function () { return []; }, function () {});
  get(documentProto, 'readyState', function () { return readyState; });
  get(documentProto, 'URL', function () { return global.location.href; });
  get(documentProto, 'documentURI', function () { return global.location.href; });
  get(documentProto, 'baseURI', function () { return global.location.href; });
  get(documentProto, 'referrer', function () { return ''; });
  get(documentProto, 'domain', function () { return global.location.hostname; }, function () {});
  get(documentProto, 'characterSet', function () { return 'UTF-8'; });
  get(documentProto, 'charset', function () { return 'UTF-8'; });
  get(documentProto, 'inputEncoding', function () { return 'UTF-8'; });
  get(documentProto, 'contentType', function () { return 'text/html'; });
  get(documentProto, 'compatMode', function () { return 'CSS1Compat'; });
  get(documentProto, 'hidden', function () { return false; });
  get(documentProto, 'visibilityState', function () { return 'visible'; });
  get(documentProto, 'activeElement', function () { return activeElement || this.body; });
  get(documentProto, 'defaultView', function () { return global; });
  get(documentProto, 'doctype', function () {
    for (let c = this.firstChild; c; c = c.nextSibling) if (c.nodeType === 10) return c;
    return null;
  });
  get(documentProto, 'cookie', function () { return native.cookieGet(); }, function (value) { native.cookieSet(String(value)); });
  get(documentProto, 'scrollingElement', function () { return this.documentElement; });
  get(documentProto, 'fullscreenElement', function () { return null; });
  get(documentProto, 'fullscreenEnabled', function () { return false; });
  get(documentProto, 'pointerLockElement', function () { return null; });
  get(documentProto, 'currentScript', function () { return native.currentScript(); });
  get(documentProto, 'implementation', function () {
    return {
      createHTMLDocument(title) {
        const doc = native.parseDocument('<!doctype html><title>' + (title || '') + '</title><body>');
        return doc;
      },
      hasFeature() { return true; },
    };
  });
  get(documentProto, 'fonts', function () {
    return { ready: Promise.resolve(), status: 'loaded', load: () => Promise.resolve([]), check: () => true, add() {}, delete() {}, addEventListener() {}, removeEventListener() {}, forEach() {} };
  });
  get(documentProto, 'timeline', function () { return { currentTime: 0 }; });

  function makeStyleSheet(element) {
    const rules = [];
    return {
      ownerNode: element, href: null, disabled: false, type: 'text/css', media: { mediaText: '' }, cssRules: rules, rules,
      insertRule(rule, index) {
        element.textContent = (element.textContent || '') + '\n' + rule;
        rules.splice(index === undefined ? 0 : index, 0, { cssText: rule });
        return index || 0;
      },
      deleteRule(index) { rules.splice(index, 1); },
    };
  }
  global.CSSStyleSheet = function () {
    return { cssRules: [], insertRule() { return 0; }, deleteRule() {}, replace() { return Promise.resolve(this); }, replaceSync() {} };
  };
  global.CSS = {
    supports: (a, b) => native.cssSupports(b === undefined ? String(a) : '(' + a + ':' + b + ')'),
    escape: (text) => String(text).replace(/[^a-zA-Z0-9_\-\u0080-￿]/g, (c) => '\\' + c),
  };
  global.DOMParser = function () {};
  global.DOMParser.prototype.parseFromString = function (text) { return native.parseDocument(String(text)); };
  global.XMLSerializer = function () {};
  global.XMLSerializer.prototype.serializeToString = function (node) { return node.outerHTML || ''; };
  global.DOMException = class DOMException extends Error {
    constructor(message = '', name = 'Error') { super(message); this.name = name; this.code = 0; }
  };

  // ---- Custom elements ---------------------------------------------------

  function upgrade(element, definition) {
    if (element.__upgraded) return;
    hide(element, '__upgraded', true);
    constructing = element;
    try { new definition.ctor(); } catch (error) { reportError(error); }
    constructing = null;
    if (definition.ctor.observedAttributes && element.attributeChangedCallback) {
      for (const name of definition.ctor.observedAttributes) {
        if (element.hasAttribute(name)) {
          try { element.attributeChangedCallback(name, null, element.getAttribute(name)); } catch (error) { reportError(error); }
        }
      }
    }
    if (element.isConnected && element.connectedCallback) {
      try { element.connectedCallback(); } catch (error) { reportError(error); }
    }
  }
  const waiters = new Map();
  global.customElements = {
    define(name, ctor) {
      if (customRegistry.has(name)) throw new DOMException(name + ' is already defined', 'NotSupportedError');
      const definition = { ctor };
      customRegistry.set(name, definition);
      for (const element of document.querySelectorAll(name)) upgrade(element, definition);
      const waiter = waiters.get(name);
      if (waiter) waiter.resolve(ctor);
    },
    get(name) { const d = customRegistry.get(name); return d ? d.ctor : undefined; },
    getName(ctor) { for (const [name, d] of customRegistry) if (d.ctor === ctor) return name; return null; },
    upgrade(root) {
      for (const [name, definition] of customRegistry) for (const element of root.querySelectorAll(name)) upgrade(element, definition);
    },
    whenDefined(name) {
      const d = customRegistry.get(name);
      if (d) return Promise.resolve(d.ctor);
      let waiter = waiters.get(name);
      if (!waiter) {
        waiter = {};
        waiter.promise = new Promise((resolve) => { waiter.resolve = resolve; });
        waiters.set(name, waiter);
      }
      return waiter.promise;
    },
  };

  // ---- Window ------------------------------------------------------------

  define(global, 'innerWidth', { get() { return native.viewport()[0]; } });
  define(global, 'innerHeight', { get() { return native.viewport()[1]; } });
  define(global, 'outerWidth', { get() { return native.viewport()[0]; } });
  define(global, 'outerHeight', { get() { return native.viewport()[1]; } });
  global.devicePixelRatio = 1;
  global.screenX = global.screenY = global.screenLeft = global.screenTop = 0;
  global.scrollX = global.scrollY = global.pageXOffset = global.pageYOffset = 0;
  define(global, 'screen', {
    get() {
      const [width, height] = native.viewport();
      return { width, height, availWidth: width, availHeight: height, colorDepth: 24, pixelDepth: 24, orientation: { type: 'landscape-primary', angle: 0, addEventListener() {}, removeEventListener() {} } };
    },
  });
  global.visualViewport = {
    get width() { return global.innerWidth; }, get height() { return global.innerHeight; }, scale: 1, offsetLeft: 0, offsetTop: 0,
    addEventListener() {}, removeEventListener() {},
  };
  for (const name of ['scroll', 'scrollTo', 'scrollBy', 'focus', 'blur', 'print', 'stop', 'moveTo', 'resizeTo']) global[name] = function () {};
  global.alert = function () {};
  global.confirm = function () { return false; };
  global.prompt = function () { return null; };
  global.open = function () { return null; };
  global.close = function () {};
  global.getSelection = function () {
    return { rangeCount: 0, isCollapsed: true, anchorNode: null, focusNode: null, toString: () => '', removeAllRanges() {}, addRange() {}, collapse() {}, getRangeAt() { return document.createRange(); }, type: 'None' };
  };
  global.getComputedStyle = function (element, pseudo) {
    const values = (!pseudo && element && native.computedStyle(element)) || {};
    const style = {
      getPropertyValue(name) { return values[name] !== undefined ? values[name] : ''; },
      getPropertyPriority() { return ''; },
      item(i) { return Object.keys(values)[i] || ''; },
      get length() { return Object.keys(values).length; },
    };
    for (const name of Object.keys(values)) {
      style[name.replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = values[name];
      style[name] = values[name];
    }
    return style;
  };
  global.matchMedia = function (query) {
    const text = String(query);
    const list = {
      media: text,
      get matches() { return native.matchMedia(text); },
      onchange: null,
      addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; },
    };
    return list;
  };
  const queueMicrotaskImpl = (callback) => { Promise.resolve().then(callback).catch(reportError); };
  global.queueMicrotask = queueMicrotaskImpl;
  global.requestIdleCallback = (callback) => setTimeout(() => callback({ didTimeout: false, timeRemaining: () => 10 }), 1);
  global.cancelIdleCallback = (id) => clearTimeout(id);

  // Same-window messages are delivered to the page as well as to the browser.
  const capturePostMessage = global.postMessage;
  global.postMessage = function (message, target) {
    capturePostMessage(message, target);
    setTimeout(() => {
      global.dispatchEvent(new MessageEvent('message', { data: message, origin: global.location.origin, source: global }));
    }, 0);
  };
  global.MessageChannel = function () {
    const makePort = () => ({
      onmessage: null, other: null, __listeners: new Map(),
      addEventListener: EventTarget.prototype.addEventListener,
      removeEventListener: EventTarget.prototype.removeEventListener,
      start() {}, close() {},
      postMessage(data) {
        const other = this.other;
        setTimeout(() => {
          const event = new MessageEvent('message', { data });
          fireAt(other, event);
        }, 0);
      },
    });
    this.port1 = makePort();
    this.port2 = makePort();
    this.port1.other = this.port2;
    this.port2.other = this.port1;
  };
  global.BroadcastChannel = function (name) {
    this.name = name;
    this.postMessage = () => {};
    this.close = () => {};
    this.addEventListener = () => {};
    this.removeEventListener = () => {};
    this.onmessage = null;
  };

  // Location beyond what's native.
  define(location, 'origin', { get() { const p = native.parseUrl(location.href); return p ? p.origin : 'null'; } });
  define(location, 'port', { get() { const p = native.parseUrl(location.href); return p ? p.port : ''; } });
  hide(location, 'assign', function (url) { native.navigate(String(url)); });
  hide(location, 'replace', function (url) { native.navigate(String(url)); });
  hide(location, 'reload', function () { native.navigate(location.href); });
  hide(location, 'toString', function () { return location.href; });
  define(location, 'ancestorOrigins', { get() { return []; } });

  const historyEntries = [{ state: null, url: null }];
  let historyIndex = 0;
  const applyHistoryUrl = (url) => {
    if (url === undefined || url === null) return;
    const parsed = native.parseUrl(String(url), location.href);
    if (parsed) native.setLocationSilently(parsed.href);
  };
  global.history = {
    get length() { return historyEntries.length; },
    get state() { return historyEntries[historyIndex].state; },
    scrollRestoration: 'auto',
    pushState(state, _title, url) {
      applyHistoryUrl(url);
      historyEntries.splice(historyIndex + 1);
      historyEntries.push({ state, url: location.href });
      historyIndex = historyEntries.length - 1;
    },
    replaceState(state, _title, url) {
      applyHistoryUrl(url);
      historyEntries[historyIndex] = { state, url: location.href };
    },
    go(delta = 0) {
      const next = historyIndex + delta;
      if (next < 0 || next >= historyEntries.length) return;
      historyIndex = next;
      const entry = historyEntries[next];
      if (entry.url) native.setLocationSilently(entry.url);
      setTimeout(() => global.dispatchEvent(new PopStateEvent('popstate', { state: entry.state })), 0);
    },
    back() { this.go(-1); },
    forward() { this.go(1); },
  };
  global.History = function () {};

  const userAgent = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) CosmoBrowse/0.1 Safari/537.36';
  global.navigator = {
    userAgent, appVersion: userAgent.slice(8), appName: 'Netscape', appCodeName: 'Mozilla', product: 'Gecko', productSub: '20030107',
    vendor: '', vendorSub: '', platform: 'Linux x86_64', language: 'en-US', languages: ['en-US', 'en'], onLine: true,
    cookieEnabled: true, doNotTrack: null, hardwareConcurrency: 4, maxTouchPoints: 0, pdfViewerEnabled: false, webdriver: false,
    deviceMemory: 8, plugins: [], mimeTypes: [],
    javaEnabled: () => false,
    sendBeacon: () => true,
    vibrate: () => false,
    getGamepads: () => [],
    clipboard: { writeText: () => Promise.resolve(), readText: () => Promise.resolve(''), write: () => Promise.resolve() },
    permissions: { query: () => Promise.resolve({ state: 'prompt', addEventListener() {}, removeEventListener() {} }) },
    storage: { estimate: () => Promise.resolve({ usage: 0, quota: 0 }), persist: () => Promise.resolve(false), persisted: () => Promise.resolve(false) },
  };
  global.Navigator = function () {};

  const makeStorage = () => {
    const entries = new Map();
    return {
      getItem: (key) => (entries.has(String(key)) ? entries.get(String(key)) : null),
      setItem: (key, value) => { entries.set(String(key), String(value)); },
      removeItem: (key) => { entries.delete(String(key)); },
      clear: () => { entries.clear(); },
      key: (index) => Array.from(entries.keys())[index] || null,
      get length() { return entries.size; },
    };
  };
  global.sessionStorage = makeStorage();
  global.Storage = function () {};

  const startTime = Date.now();
  const marks = [];
  global.performance = {
    now: () => Date.now() - startTime,
    timeOrigin: startTime,
    mark(name) { const entry = { name, entryType: 'mark', startTime: Date.now() - startTime, duration: 0 }; marks.push(entry); return entry; },
    measure(name) { const entry = { name, entryType: 'measure', startTime: 0, duration: Date.now() - startTime }; marks.push(entry); return entry; },
    clearMarks() { marks.length = 0; }, clearMeasures() {}, clearResourceTimings() {}, setResourceTimingBufferSize() {},
    getEntries: () => marks.slice(),
    getEntriesByName: (name) => marks.filter((m) => m.name === name),
    getEntriesByType: (type) => marks.filter((m) => m.entryType === type),
    timing: { navigationStart: startTime, loadEventEnd: 0, domContentLoadedEventEnd: 0 },
    navigation: { type: 0, redirectCount: 0 },
    memory: undefined,
    toJSON() { return {}; },
  };
  global.PerformanceObserver = function () { this.observe = () => {}; this.disconnect = () => {}; this.takeRecords = () => []; };
  global.PerformanceObserver.supportedEntryTypes = [];

  global.crypto = {
    getRandomValues(array) {
      for (let i = 0; i < array.length; i++) array[i] = Math.floor(Math.random() * 4294967296);
      return array;
    },
    randomUUID() {
      const hex = () => Math.floor(Math.random() * 16).toString(16);
      return 'xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx'.replace(/x/g, hex);
    },
  };

  const base64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  global.btoa = function (text) {
    const s = String(text);
    let out = '';
    for (let i = 0; i < s.length; i += 3) {
      const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new DOMException('Invalid character', 'InvalidCharacterError');
      out += base64[a >> 2] + base64[((a & 3) << 4) | ((b || 0) >> 4)];
      out += Number.isNaN(b) ? '==' : base64[((b & 15) << 2) | ((c || 0) >> 6)] + (Number.isNaN(c) ? '=' : base64[c & 63]);
    }
    return out;
  };
  global.atob = function (text) {
    const s = String(text).replace(/[\s=]+/g, '');
    let out = '', bits = 0, count = 0;
    for (const ch of s) {
      const value = base64.indexOf(ch);
      if (value < 0) throw new DOMException('Invalid character', 'InvalidCharacterError');
      bits = (bits << 6) | value;
      count += 6;
      if (count >= 8) { count -= 8; out += String.fromCharCode((bits >> count) & 255); }
    }
    return out;
  };

  global.TextEncoder = class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(text = '') {
      const out = [];
      for (const ch of String(text)) {
        const c = ch.codePointAt(0);
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
        else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
        else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      }
      return new Uint8Array(out);
    }
  };
  global.TextDecoder = class TextDecoder {
    constructor(label = 'utf-8') { this.encoding = String(label).toLowerCase(); this.fatal = false; this.ignoreBOM = false; }
    decode(input) {
      if (!input) return '';
      const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : new Uint8Array(input.buffer || input);
      let out = '', i = 0;
      while (i < bytes.length) {
        const b = bytes[i++];
        if (b < 0x80) out += String.fromCharCode(b);
        else if (b < 0xe0) out += String.fromCharCode(((b & 31) << 6) | (bytes[i++] & 63));
        else if (b < 0xf0) out += String.fromCharCode(((b & 15) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63));
        else out += String.fromCodePoint(((b & 7) << 18) | ((bytes[i++] & 63) << 12) | ((bytes[i++] & 63) << 6) | (bytes[i++] & 63));
      }
      return out;
    }
  };

  // ---- URL ---------------------------------------------------------------

  class URLSearchParams {
    constructor(init = '') {
      this.__pairs = [];
      this.__url = null;
      if (typeof init === 'string') {
        const text = init.startsWith('?') ? init.slice(1) : init;
        for (const part of text.split('&')) {
          if (!part) continue;
          const at = part.indexOf('=');
          const name = at < 0 ? part : part.slice(0, at);
          const value = at < 0 ? '' : part.slice(at + 1);
          this.__pairs.push([decodeForm(name), decodeForm(value)]);
        }
      } else if (init && typeof init[Symbol.iterator] === 'function') {
        for (const [name, value] of init) this.__pairs.push([String(name), String(value)]);
      } else if (init && typeof init === 'object') {
        for (const name of Object.keys(init)) this.__pairs.push([name, String(init[name])]);
      }
    }
    __changed() { if (this.__url) this.__url.__setSearch(this.toString()); }
    append(name, value) { this.__pairs.push([String(name), String(value)]); this.__changed(); }
    delete(name) { this.__pairs = this.__pairs.filter((p) => p[0] !== String(name)); this.__changed(); }
    get(name) { const p = this.__pairs.find((q) => q[0] === String(name)); return p ? p[1] : null; }
    getAll(name) { return this.__pairs.filter((p) => p[0] === String(name)).map((p) => p[1]); }
    has(name) { return this.__pairs.some((p) => p[0] === String(name)); }
    set(name, value) {
      const key = String(name);
      const index = this.__pairs.findIndex((p) => p[0] === key);
      if (index < 0) this.__pairs.push([key, String(value)]);
      else {
        this.__pairs[index][1] = String(value);
        this.__pairs = this.__pairs.filter((p, i) => p[0] !== key || i === index);
      }
      this.__changed();
    }
    sort() { this.__pairs.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); this.__changed(); }
    forEach(callback, thisArg) { for (const [name, value] of this.__pairs) callback.call(thisArg, value, name, this); }
    keys() { return this.__pairs.map((p) => p[0])[Symbol.iterator](); }
    values() { return this.__pairs.map((p) => p[1])[Symbol.iterator](); }
    entries() { return this.__pairs.map((p) => [p[0], p[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get size() { return this.__pairs.length; }
    toString() { return this.__pairs.map((p) => encodeForm(p[0]) + '=' + encodeForm(p[1])).join('&'); }
  }
  function decodeForm(text) {
    const spaced = text.replace(/\+/g, ' ');
    try { return decodeURIComponent(spaced); } catch (_) { return spaced; }
  }
  function encodeForm(text) {
    return encodeURIComponent(text).replace(/%20/g, '+').replace(/[!'()~]/g, (c) => '%' + c.charCodeAt(0).toString(16).toUpperCase());
  }
  global.URLSearchParams = URLSearchParams;

  class URL {
    constructor(url, base) {
      const parsed = native.parseUrl(String(url), base === undefined ? undefined : String(base));
      if (!parsed) throw new TypeError('Invalid URL: ' + url);
      this.__parts = parsed;
      this.__params = null;
    }
    static canParse(url, base) { return !!native.parseUrl(String(url), base === undefined ? undefined : String(base)); }
    static parse(url, base) { try { return new URL(url, base); } catch (_) { return null; } }
    static createObjectURL() { return 'blob:cosmo'; }
    static revokeObjectURL() {}
    __reparse() { const p = native.parseUrl(this.__parts.href); if (p) this.__parts = p; }
    __setSearch(query) {
      const parts = this.__parts;
      const search = query ? '?' + query : '';
      this.href = parts.protocol + '//' + (parts.username ? parts.username + (parts.password ? ':' + parts.password : '') + '@' : '') + parts.host + parts.pathname + search + parts.hash;
    }
    get href() { return this.__parts.href; }
    set href(value) {
      const parsed = native.parseUrl(String(value));
      if (!parsed) throw new TypeError('Invalid URL: ' + value);
      this.__parts = parsed;
    }
    get origin() { return this.__parts.origin; }
    get protocol() { return this.__parts.protocol; }
    get username() { return this.__parts.username; }
    get password() { return this.__parts.password; }
    get host() { return this.__parts.host; }
    get hostname() { return this.__parts.hostname; }
    get port() { return this.__parts.port; }
    get pathname() { return this.__parts.pathname; }
    set pathname(value) {
      const path = String(value);
      const parts = this.__parts;
      this.href = parts.protocol + '//' + parts.host + (path.startsWith('/') ? path : '/' + path) + parts.search + parts.hash;
    }
    get search() { return this.__parts.search; }
    set search(value) {
      const text = String(value);
      this.__setSearch(text.startsWith('?') ? text.slice(1) : text);
      this.__params = null;
    }
    get hash() { return this.__parts.hash; }
    set hash(value) {
      const text = String(value);
      const parts = this.__parts;
      this.href = parts.protocol + '//' + parts.host + parts.pathname + parts.search + (text && !text.startsWith('#') ? '#' + text : text);
    }
    get searchParams() {
      if (!this.__params) {
        this.__params = new URLSearchParams(this.search);
        this.__params.__url = this;
      }
      return this.__params;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }
  global.URL = URL;
  global.webkitURL = URL;

  // ---- Data --------------------------------------------------------------

  global.Blob = class Blob {
    constructor(parts = [], options = {}) {
      this.__text = parts.map((p) => (p && p.__text !== undefined ? p.__text : typeof p === 'string' ? p : String(p))).join('');
      this.type = options.type || '';
    }
    get size() { return new TextEncoder().encode(this.__text).length; }
    text() { return Promise.resolve(this.__text); }
    arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this.__text).buffer); }
    slice(start, end, type) { return new Blob([this.__text.slice(start, end)], { type }); }
  };
  global.File = class File extends global.Blob {
    constructor(parts, name, options = {}) {
      super(parts, options);
      this.name = String(name);
      this.lastModified = options.lastModified || Date.now();
    }
  };
  global.FileReader = class FileReader extends EventTarget {
    constructor() { super(); this.result = null; this.readyState = 0; this.error = null; }
    readAsText(blob) { this.__finish(blob.text()); }
    readAsDataURL(blob) { this.__finish(blob.text().then((t) => 'data:' + blob.type + ';base64,' + btoa(t))); }
    readAsArrayBuffer(blob) { this.__finish(blob.arrayBuffer()); }
    __finish(promise) {
      promise.then((result) => {
        this.result = result;
        this.readyState = 2;
        this.dispatchEvent(new ProgressEvent('load'));
        this.dispatchEvent(new ProgressEvent('loadend'));
      });
    }
    abort() {}
  };

  global.FormData = class FormData {
    constructor(form) {
      this.__pairs = [];
      if (form) for (const [name, value] of native.formData(form)) this.__pairs.push([name, value]);
    }
    append(name, value) { this.__pairs.push([String(name), value instanceof global.Blob ? value : String(value)]); }
    delete(name) { this.__pairs = this.__pairs.filter((p) => p[0] !== String(name)); }
    get(name) { const p = this.__pairs.find((q) => q[0] === String(name)); return p ? p[1] : null; }
    getAll(name) { return this.__pairs.filter((p) => p[0] === String(name)).map((p) => p[1]); }
    has(name) { return this.__pairs.some((p) => p[0] === String(name)); }
    set(name, value) { this.delete(name); this.append(name, value); }
    forEach(callback, thisArg) { for (const [name, value] of this.__pairs) callback.call(thisArg, value, name, this); }
    keys() { return this.__pairs.map((p) => p[0])[Symbol.iterator](); }
    values() { return this.__pairs.map((p) => p[1])[Symbol.iterator](); }
    entries() { return this.__pairs.map((p) => [p[0], p[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  };

  global.AbortSignal = class AbortSignal extends EventTarget {
    constructor() { super(); this.aborted = false; this.reason = undefined; this.onabort = null; }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    static abort(reason) { const s = new AbortSignal(); s.aborted = true; s.reason = reason; return s; }
    static timeout(ms) {
      const s = new AbortSignal();
      setTimeout(() => { s.aborted = true; s.reason = new DOMException('Timed out', 'TimeoutError'); s.dispatchEvent(new Event('abort')); }, ms);
      return s;
    }
    static any() { return new AbortSignal(); }
  };
  global.AbortController = class AbortController {
    constructor() { this.signal = new global.AbortSignal(); }
    abort(reason) {
      if (this.signal.aborted) return;
      this.signal.aborted = true;
      this.signal.reason = reason === undefined ? new DOMException('Aborted', 'AbortError') : reason;
      this.signal.dispatchEvent(new Event('abort'));
    }
  };

  global.structuredClone = function (value) {
    const seen = new Map();
    const clone = (item) => {
      if (item === null || typeof item !== 'object') {
        if (typeof item === 'function' || typeof item === 'symbol') throw new DOMException('could not be cloned', 'DataCloneError');
        return item;
      }
      if (seen.has(item)) return seen.get(item);
      if (item instanceof Date) return new Date(item.getTime());
      if (item instanceof RegExp) return new RegExp(item.source, item.flags);
      if (ArrayBuffer.isView(item)) return new item.constructor(item);
      if (item instanceof ArrayBuffer) return item.slice(0);
      if (item instanceof Map) {
        const out = new Map();
        seen.set(item, out);
        for (const [k, v] of item) out.set(clone(k), clone(v));
        return out;
      }
      if (item instanceof Set) {
        const out = new Set();
        seen.set(item, out);
        for (const v of item) out.add(clone(v));
        return out;
      }
      const out = Array.isArray(item) ? [] : {};
      seen.set(item, out);
      for (const key of Object.keys(item)) out[key] = clone(item[key]);
      return out;
    };
    return clone(value);
  };

  // fetch's result is a plain object natively: give it the rest of `Response`.
  const nativeFetch = global.fetch;
  class Headers {
    constructor(init) {
      this.__map = new Map();
      if (!init) return;
      if (init instanceof Headers) init.forEach((v, k) => this.append(k, v));
      else if (Array.isArray(init)) for (const [k, v] of init) this.append(k, v);
      else if (typeof init.forEach === 'function' && typeof init.get === 'function') init.forEach((v, k) => this.append(k, v));
      else for (const key of Object.keys(init)) this.append(key, init[key]);
    }
    append(name, value) {
      const key = String(name).toLowerCase();
      this.__map.set(key, this.__map.has(key) ? this.__map.get(key) + ', ' + value : String(value));
    }
    set(name, value) { this.__map.set(String(name).toLowerCase(), String(value)); }
    get(name) { const v = this.__map.get(String(name).toLowerCase()); return v === undefined ? null : v; }
    has(name) { return this.__map.has(String(name).toLowerCase()); }
    delete(name) { this.__map.delete(String(name).toLowerCase()); }
    forEach(callback, thisArg) { for (const [k, v] of this.__map) callback.call(thisArg, v, k, this); }
    keys() { return this.__map.keys(); }
    values() { return this.__map.values(); }
    entries() { return this.__map.entries(); }
    [Symbol.iterator]() { return this.__map.entries(); }
  }
  global.Headers = Headers;
  class Response {
    constructor(body = null, init = {}) {
      this.__body = body === null || body === undefined ? '' : String(body);
      this.status = init.status === undefined ? 200 : init.status;
      this.statusText = init.statusText || '';
      this.ok = this.status >= 200 && this.status < 300;
      this.headers = new Headers(init.headers);
      this.url = init.url || '';
      this.type = 'basic';
      this.redirected = false;
      this.bodyUsed = false;
    }
    text() { this.bodyUsed = true; return Promise.resolve(this.__body); }
    json() { this.bodyUsed = true; return Promise.resolve(this.__body).then(JSON.parse); }
    blob() { return Promise.resolve(new global.Blob([this.__body])); }
    arrayBuffer() { return Promise.resolve(new TextEncoder().encode(this.__body).buffer); }
    formData() { return Promise.resolve(new global.FormData()); }
    clone() { return new Response(this.__body, this); }
    get body() { return null; }
    static json(data, init) { return new Response(JSON.stringify(data), init); }
    static error() { return new Response('', { status: 0 }); }
  }
  global.Response = Response;
  class Request {
    constructor(input, init = {}) {
      this.url = input instanceof Request ? input.url : new URL(String(input), location.href).href;
      this.method = (init.method || (input instanceof Request && input.method) || 'GET').toUpperCase();
      this.headers = new Headers(init.headers || (input instanceof Request ? input.headers : undefined));
      this.__body = init.body !== undefined ? init.body : input instanceof Request ? input.__body : undefined;
      this.signal = init.signal || null;
      this.mode = init.mode || 'cors';
      this.credentials = init.credentials || 'same-origin';
      this.cache = init.cache || 'default';
      this.redirect = init.redirect || 'follow';
      this.referrer = '';
    }
    clone() { return new Request(this); }
    text() { return Promise.resolve(this.__body === undefined ? '' : String(this.__body)); }
    json() { return this.text().then(JSON.parse); }
  }
  global.Request = Request;
  global.fetch = function (input, init = {}) {
    const request = input instanceof Request ? input : null;
    const url = request ? request.url : String(input && input.href ? input.href : input);
    const options = Object.assign({}, init);
    if (request) {
      options.method = options.method || request.method;
      if (options.body === undefined) options.body = request.__body;
      if (options.headers === undefined) options.headers = request.headers;
    }
    if (options.headers instanceof Headers) {
      const plain = {};
      options.headers.forEach((v, k) => { plain[k] = v; });
      options.headers = plain;
    }
    if (options.body instanceof global.FormData || options.body instanceof URLSearchParams) {
      const params = new URLSearchParams();
      for (const [k, v] of options.body) params.append(k, v);
      options.body = params.toString();
      options.headers = Object.assign({ 'content-type': 'application/x-www-form-urlencoded' }, options.headers);
    } else if (options.body instanceof global.Blob) {
      options.body = options.body.__text;
    }
    const signal = options.signal || (request && request.signal);
    if (signal && signal.aborted) return Promise.reject(signal.reason || new DOMException('Aborted', 'AbortError'));
    delete options.signal;
    return nativeFetch(url, options).then((response) => response.text().then((body) => new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers && typeof response.headers.get === 'function' ? response.headers : responseHeaders(response.headers),
      url: response.url || url,
    })));
  };
  function responseHeaders(headers) {
    return headers && typeof headers === 'object' ? headers : undefined;
  }

  // ---- Observers ---------------------------------------------------------

  // These report what they can without being driven by layout: an observed
  // element counts as visible and at its current size, once. It's enough for
  // lazy loading and for code that measures at start-up, and nothing more.
  global.IntersectionObserver = class IntersectionObserver {
    constructor(callback, options = {}) {
      this.__callback = callback;
      this.root = options.root || null;
      this.rootMargin = options.rootMargin || '0px';
      this.thresholds = [].concat(options.threshold === undefined ? 0 : options.threshold);
      this.__seen = new WeakSet();
    }
    observe(target) {
      if (this.__seen.has(target)) return;
      this.__seen.add(target);
      setTimeout(() => {
        const rect = target.getBoundingClientRect();
        const entry = {
          target, isIntersecting: true, intersectionRatio: 1, time: performance.now(),
          boundingClientRect: rect, intersectionRect: rect, rootBounds: null,
        };
        try { this.__callback([entry], this); } catch (error) { reportError(error); }
      }, 0);
    }
    unobserve(target) { this.__seen.delete(target); }
    disconnect() { this.__seen = new WeakSet(); }
    takeRecords() { return []; }
  };
  global.ResizeObserver = class ResizeObserver {
    constructor(callback) { this.__callback = callback; this.__seen = new WeakSet(); }
    observe(target) {
      if (this.__seen.has(target)) return;
      this.__seen.add(target);
      setTimeout(() => {
        const rect = target.getBoundingClientRect();
        const size = [{ inlineSize: rect.width, blockSize: rect.height }];
        const entry = { target, contentRect: rect, borderBoxSize: size, contentBoxSize: size, devicePixelContentBoxSize: size };
        try { this.__callback([entry], this); } catch (error) { reportError(error); }
      }, 0);
    }
    unobserve(target) { this.__seen.delete(target); }
    disconnect() { this.__seen = new WeakSet(); }
  };
  // Records child list changes by comparing, on the next task after each change
  // the engine reports, what's been observed.
  global.MutationObserver = class MutationObserver {
    constructor(callback) { this.__callback = callback; this.__targets = []; this.__generation = native.domGeneration(); }
    observe(target, options = {}) {
      this.__targets.push({ target, options });
      activeObservers.add(this);
      scheduleMutationCheck();
    }
    disconnect() { this.__targets = []; activeObservers.delete(this); }
    takeRecords() { return []; }
    __deliver() {
      const generation = native.domGeneration();
      if (generation === this.__generation || !this.__targets.length) return;
      this.__generation = generation;
      const records = this.__targets.map(({ target }) => ({
        type: 'childList', target, addedNodes: [], removedNodes: [], previousSibling: null, nextSibling: null,
        attributeName: null, attributeNamespace: null, oldValue: null,
      }));
      try { this.__callback(records, this); } catch (error) { reportError(error); }
      this.__generation = native.domGeneration();
    }
  };
  const activeObservers = new Set();
  let mutationCheckScheduled = false;
  function scheduleMutationCheck() {
    if (mutationCheckScheduled) return;
    mutationCheckScheduled = true;
    setTimeout(function check() {
      mutationCheckScheduled = false;
      for (const observer of activeObservers) observer.__deliver();
      if (activeObservers.size) scheduleMutationCheck();
    }, 16);
  }

  // ---- Stand-ins for what isn't here -------------------------------------

  const closedSocket = (name) => class extends EventTarget {
    constructor(url) {
      super();
      this.url = String(url);
      this.readyState = 3;
      this.CONNECTING = 0; this.OPEN = 1; this.CLOSING = 2; this.CLOSED = 3;
      setTimeout(() => {
        this.dispatchEvent(new Event('error'));
        this.dispatchEvent(new Event('close'));
      }, 0);
    }
    send() {}
    close() {}
  };
  global.WebSocket = closedSocket('WebSocket');
  global.EventSource = closedSocket('EventSource');
  global.Worker = class Worker extends EventTarget {
    constructor() { super(); setTimeout(() => this.dispatchEvent(new ErrorEvent('error', { message: 'Workers are not supported' })), 0); }
    postMessage() {}
    terminate() {}
  };

  // No ICU data: English-only formatting that keeps code using these working.
  if (typeof global.Intl === 'undefined') {
    const groupThousands = (text) => text.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    class NumberFormat {
      constructor(_locale, options = {}) { this.options = options; }
      format(value) {
        const number = Number(value);
        const { minimumFractionDigits: min, maximumFractionDigits: max, style, currency } = this.options;
        const digits = max === undefined ? (min === undefined ? (Number.isInteger(number) ? 0 : 3) : min) : max;
        let text = number.toFixed(digits);
        if (max !== undefined && (min === undefined || min < max)) text = text.replace(/\.?0+$/, '');
        const [whole, fraction] = text.split('.');
        let out = groupThousands(whole) + (fraction ? '.' + fraction : '');
        if (style === 'percent') out = groupThousands(String(Math.round(number * 100))) + '%';
        if (style === 'currency') out = (currency === 'USD' ? '$' : (currency || '') + ' ') + out;
        return out;
      }
      formatToParts(value) { return [{ type: 'literal', value: this.format(value) }]; }
      resolvedOptions() { return Object.assign({ locale: 'en-US', numberingSystem: 'latn' }, this.options); }
    }
    class DateTimeFormat {
      constructor(_locale, options = {}) { this.options = options; }
      format(date) { return (date === undefined ? new Date() : new Date(date)).toLocaleDateString(); }
      formatToParts(date) { return [{ type: 'literal', value: this.format(date) }]; }
      resolvedOptions() { return { locale: 'en-US', calendar: 'gregory', numberingSystem: 'latn', timeZone: 'UTC' }; }
    }
    global.Intl = {
      NumberFormat, DateTimeFormat,
      Collator: class { compare(a, b) { return String(a) < String(b) ? -1 : String(a) > String(b) ? 1 : 0; } resolvedOptions() { return { locale: 'en-US' }; } },
      PluralRules: class { select(n) { return n === 1 ? 'one' : 'other'; } resolvedOptions() { return { locale: 'en-US' }; } },
      RelativeTimeFormat: class { format(value, unit) { return Math.abs(value) + ' ' + unit + (Math.abs(value) === 1 ? '' : 's') + (value < 0 ? ' ago' : ''); } },
      ListFormat: class { format(items) { return Array.from(items).join(', '); } },
      Segmenter: class { segment(text) { return Array.from(String(text)).map((segment, index) => ({ segment, index, input: text })); } },
      DisplayNames: class { of(code) { return code; } },
      Locale: class { constructor(tag) { this.baseName = tag; this.language = String(tag).split('-')[0]; } },
      getCanonicalLocales: (locales) => [].concat(locales || []),
      supportedValuesOf: () => [],
    };
  }
})(this);
