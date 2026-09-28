//! Shape corpus for [`super::scan_source`] — written before the scanner.
//!
//! Every row is one small source file, the name it is scanned for, and the
//! exact `(line, shape)` list the scan must return. An empty list is a
//! look-alike: a comment, an unrelated string, an identifier that merely
//! contains the name, the definition itself, a direct call, an import, a
//! parameter. Rows are grouped per language; each group has both kinds.

use super::{scan_source, scan_source_with_defs, Shape};

type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static [(usize, Shape)],
);

use Shape::{EventName as E, FunctionReference as F, Reflection as R, StringKey as K, Symbol as S};

const CORPUS: &[Row] = &[
    // ---- JavaScript / TypeScript -------------------------------------------
    (
        "javascript",
        "handlers[\"save\"](doc);\n",
        "save",
        &[(1, K)],
    ),
    (
        "javascript",
        "const table = { \"save\": onSave };\n",
        "save",
        &[(1, K)],
    ),
    ("javascript", "const fn = obj[`save`];\n", "save", &[(1, K)]),
    (
        "javascript",
        "emitter.on(\"save\", handler);\n",
        "save",
        &[(1, E)],
    ),
    ("javascript", "bus.emit('save', doc)\n", "save", &[(1, E)]),
    (
        "javascript",
        "el.addEventListener(\"save\", cb);\n",
        "save",
        &[(1, E)],
    ),
    (
        "javascript",
        "await ipcRenderer.invoke(\"save\");\n",
        "save",
        &[(1, E)],
    ),
    ("javascript", "app.post(\"/x\", save);\n", "save", &[(1, F)]),
    (
        "javascript",
        "const actions = { save, load };\n",
        "save",
        &[(1, F)],
    ),
    (
        "javascript",
        "setTimeout(this.save, 10);\n",
        "save",
        &[(1, F)],
    ),
    ("javascript", "module.exports = save;\n", "save", &[(1, F)]),
    (
        "typescript",
        "export const handlers = [save, load];\n",
        "save",
        &[(1, F)],
    ),
    (
        "typescript",
        "const h = {\n  a: 1,\n  onSave: save,\n};\n",
        "save",
        &[(3, F)],
    ),
    ("javascript", "// handlers[\"save\"](doc)\n", "save", &[]),
    (
        "javascript",
        "/* emit(\"save\")\n   app.post('/', save) */\n",
        "save",
        &[],
    ),
    ("javascript", "console.log(\"save failed\");\n", "save", &[]),
    ("javascript", "log(\"save\");\n", "save", &[]),
    (
        "javascript",
        "saveAll(x); autosave(y); save_as(z); $save(q);\n",
        "save",
        &[],
    ),
    (
        "javascript",
        "function save(doc) { return 1; }\n",
        "save",
        &[],
    ),
    ("javascript", "save(doc);\n", "save", &[]),
    (
        "javascript",
        "import { save, load } from \"./store\";\n",
        "save",
        &[],
    ),
    (
        "javascript",
        "import {\n  load,\n  save,\n} from \"./store\";\n",
        "save",
        &[],
    ),
    ("javascript", "export { save };\n", "save", &[]),
    ("javascript", "export default save;\n", "save", &[]),
    (
        "javascript",
        "const { save } = require(\"./store\");\n",
        "save",
        &[],
    ),
    ("javascript", "if (this.save) { x(); }\n", "save", &[]),
    ("javascript", "const f = (a, save) => a;\n", "save", &[]),
    (
        "javascript",
        "x = cond ? \"save\" : \"load\";\n",
        "save",
        &[],
    ),
    (
        "javascript",
        "switch (a) { case \"save\": go(); }\n",
        "save",
        &[],
    ),
    ("javascript", "obj.save = function () {};\n", "save", &[]),
    ("javascript", "const x = { save: 1 };\n", "save", &[]),
    ("javascript", "const s = `${save}`;\n", "save", &[]),
    ("javascript", "if (x === save) {}\n", "save", &[]),
    // Found on express: `onerror: logerror.bind(this)` hands the function
    // over through its own `.bind` / `.call` / `.apply`.
    (
        "javascript",
        "const h = { onerror: logerror.bind(this) };\n",
        "logerror",
        &[(1, F)],
    ),
    (
        "javascript",
        "handler.call(ctx, a);\n",
        "handler",
        &[(1, F)],
    ),
    ("typescript", "fn.apply(null, args)\n", "fn", &[(1, F)]),
    ("javascript", "const n = save.length;\n", "save", &[]),
    ("python", "x = save.bind(y)\n", "save", &[]),
    (
        "typescript",
        "class A { private save(): void {} }\n",
        "save",
        &[],
    ),
    (
        "typescript",
        "type H = { save?: () => void };\n",
        "save",
        &[],
    ),
    // ---- Python ------------------------------------------------------------
    ("python", "getattr(obj, \"save\")()\n", "save", &[(1, R)]),
    (
        "python",
        "fn = getattr(self, 'save', None)\n",
        "save",
        &[(1, R)],
    ),
    (
        "python",
        "if hasattr(obj, \"save\"):\n    pass\n",
        "save",
        &[(1, R)],
    ),
    (
        "python",
        "call = operator.methodcaller(\"save\")\n",
        "save",
        &[(1, R)],
    ),
    (
        "python",
        "HANDLERS = {\"save\": save, \"load\": load}\n",
        "save",
        &[(1, K)],
    ),
    ("python", "handlers[\"save\"](doc)\n", "save", &[(1, K)]),
    (
        "python",
        "threading.Thread(target=save).start()\n",
        "save",
        &[(1, F)],
    ),
    ("python", "callbacks.append(self.save)\n", "save", &[(1, F)]),
    ("python", "handler = save\nrun()\n", "save", &[(1, F)]),
    ("python", "# getattr(obj, \"save\")\n", "save", &[]),
    (
        "python",
        "def f():\n    \"\"\"Call save to persist; getattr(x, \"save\").\"\"\"\n",
        "save",
        &[],
    ),
    ("python", "print(\"save\")\n", "save", &[]),
    ("python", "def save(self, doc):\n    pass\n", "save", &[]),
    ("python", "def run(self, save):\n    pass\n", "save", &[]),
    (
        "python",
        "def run(self, save=True):\n    pass\n",
        "save",
        &[],
    ),
    ("python", "run(save=True)\n", "save", &[]),
    ("python", "self.save(doc)\n", "save", &[]),
    ("python", "from store import save\n", "save", &[]),
    (
        "python",
        "from store import (\n    load,\n    save,\n)\n",
        "save",
        &[],
    ),
    ("python", "if save:\n    pass\n", "save", &[]),
    ("python", "autosave = 1\nsave_all()\n", "save", &[]),
    ("python", "x = \"save\"\n", "save", &[]),
    ("python", "@save\ndef g():\n    pass\n", "save", &[]),
    ("python", "s = f\"{save}\"\n", "save", &[]),
    // ---- Ruby --------------------------------------------------------------
    ("ruby", "obj.send(:save)\n", "save", &[(1, R)]),
    ("ruby", "obj.public_send(:save, x)\n", "save", &[(1, R)]),
    ("ruby", "m = method(:save)\n", "save", &[(1, R)]),
    (
        "ruby",
        "return unless respond_to?(:save)\n",
        "save",
        &[(1, R)],
    ),
    ("ruby", "obj.send \"save\"\n", "save", &[(1, R)]),
    ("ruby", "before_action :save\n", "save", &[(1, S)]),
    (
        "ruby",
        "HANDLERS = { \"save\" => :handle }\n",
        "save",
        &[(1, K)],
    ),
    ("ruby", "# obj.send(:save)\n", "save", &[]),
    ("ruby", "def save(record)\nend\n", "save", &[]),
    ("ruby", "record.save\nfoo(save)\n", "save", &[]),
    (
        "ruby",
        "before_action :save!\nobj.send(:saved?)\n",
        "save",
        &[],
    ),
    ("ruby", "Foo::save\n", "save", &[]),
    ("ruby", "opts = { save: true }\n", "save", &[]),
    ("ruby", "puts \"save\"\n", "save", &[]),
    // ---- Go ----------------------------------------------------------------
    (
        "go",
        "v.MethodByName(\"Save\").Call(nil)\n",
        "Save",
        &[(1, R)],
    ),
    (
        "go",
        "http.HandleFunc(\"/save\", Save)\n",
        "Save",
        &[(1, F)],
    ),
    (
        "go",
        "var handlers = map[string]func(){\"Save\": Save}\n",
        "Save",
        &[(1, K)],
    ),
    ("go", "sort.Slice(xs, less)\n", "less", &[(1, F)]),
    ("go", "// v.MethodByName(\"Save\")\n", "Save", &[]),
    ("go", "func Save(w http.ResponseWriter) {}\n", "Save", &[]),
    (
        "go",
        "func (s *Store) Save() error {\n\treturn nil\n}\n",
        "Save",
        &[],
    ),
    ("go", "Save(w)\n", "Save", &[]),
    ("go", "fmt.Println(\"Save\")\n", "Save", &[]),
    ("go", "s := `Save`\n", "Save", &[]),
    ("go", "SaveAll(); AutoSave()\n", "Save", &[]),
    ("go", "func run(Save func()) {}\n", "Save", &[]),
    ("go", "r := 'x'\ng(less)\n", "less", &[(2, F)]),
    // ---- Rust --------------------------------------------------------------
    (
        "rust",
        "let v: Vec<_> = xs.iter().map(Self::save).collect();\n",
        "save",
        &[(1, F)],
    ),
    ("rust", "let f: fn() = save;\n", "save", &[(1, F)]),
    (
        "rust",
        "registry.insert(\"save\", save);\n",
        "save",
        &[(1, F)],
    ),
    (
        "rust",
        "let h = Handler { on_save: save };\n",
        "save",
        &[(1, F)],
    ),
    (
        "rust",
        "fn f<'a>(x: &'a str) { g(save) }\n",
        "save",
        &[(1, F)],
    ),
    ("rust", "let c = '\"'; g(save);\n", "save", &[(1, F)]),
    ("rust", "// callbacks.push(save);\n", "save", &[]),
    ("rust", "/* outer /* inner */ g(save); */\n", "save", &[]),
    (
        "rust",
        "pub fn save(&self) -> Result<()> {\n    Ok(())\n}\n",
        "save",
        &[],
    ),
    ("rust", "self.save()?;\n", "save", &[]),
    ("rust", "use crate::store::save;\n", "save", &[]),
    (
        "rust",
        "use crate::store::{\n    load,\n    save,\n};\n",
        "save",
        &[],
    ),
    ("rust", "println!(\"save\");\n", "save", &[]),
    ("rust", "let save_all = 1; autosave();\n", "save", &[]),
    ("rust", "fn run(save: bool) {}\n", "save", &[]),
    (
        "rust",
        "let s = r#\"handlers[\"save\"] g(save)\"#;\n",
        "save",
        &[],
    ),
    ("rust", "let s = \"multi\nline g(save)\";\n", "save", &[]),
    // Found by the dogfood survey on this repo: a match arm is not a hash
    // rocket, a `json!` key is data, and a pattern binding is not a value.
    (
        "rust",
        "match d { \"both\" => Some(\"both\"), _ => None }\n",
        "both",
        &[],
    ),
    ("rust", "let v = json!({\"save\": 1});\n", "save", &[]),
    ("rust", "out[\"save\"] = json!(1);\n", "save", &[]),
    ("rust", "(handlers[\"save\"])(doc);\n", "save", &[(1, K)]),
    (
        "javascript",
        "handlers[\"save\"] = save;\n",
        "save",
        &[(1, K)],
    ),
    ("rust", "let Some(save) = x else { return };\n", "save", &[]),
    ("rust", "match x { Some(save) => 1, _ => 2 }\n", "save", &[]),
    ("javascript", "const [a, save] = useState();\n", "save", &[]),
    ("python", "(a, save) = pair\n", "save", &[]),
    // ---- Java --------------------------------------------------------------
    (
        "java",
        "Method m = cls.getMethod(\"save\");\n",
        "save",
        &[(1, R)],
    ),
    (
        "java",
        "cls.getDeclaredMethod(\"save\", String.class);\n",
        "save",
        &[(1, R)],
    ),
    ("java", "list.forEach(this::save);\n", "save", &[(1, F)]),
    ("java", "executor.submit(Store::save);\n", "save", &[(1, F)]),
    ("java", "public void save(Doc d) {\n}\n", "save", &[]),
    ("java", "// cls.getMethod(\"save\")\n", "save", &[]),
    ("java", "store.save(d);\n", "save", &[]),
    ("java", "import static com.x.Store.save;\n", "save", &[]),
    ("java", "log.info(\"save\");\n", "save", &[]),
    ("java", "void run(Doc save) {}\n", "save", &[]),
    // ---- C / C++ -----------------------------------------------------------
    ("c", "void *p = dlsym(h, \"save\");\n", "save", &[(1, R)]),
    ("c", "signal(SIGINT, save);\n", "save", &[(1, F)]),
    (
        "c",
        "static const struct ops o = { .save = save, };\n",
        "save",
        &[(1, F)],
    ),
    ("c", "cb = &save;\n", "save", &[(1, F)]),
    (
        "c",
        "static const struct e t[] = {\n  {\"save\", save},\n};\n",
        "save",
        &[(2, F)],
    ),
    ("c", "char c = '\"'; g(save);\n", "save", &[(1, F)]),
    ("c", "void save(struct doc *d);\n", "save", &[]),
    ("c", "/* signal(SIGINT, save); */\n", "save", &[]),
    ("c", "#include \"save.h\"\n", "save", &[]),
    ("c", "printf(\"save\\n\");\n", "save", &[]),
    ("c", "int save_count = 0;\n", "save", &[]),
    ("c", "void (*save)(void);\n", "save", &[]),
    ("c", "x = a ? \"save\" : b;\n", "save", &[]),
    ("c", "if (a && save) {}\n", "save", &[]),
    (
        "cpp",
        "std::function<void()> f = &Store::save;\n",
        "save",
        &[(1, F)],
    ),
    (
        "cpp",
        "QMetaObject::invokeMethod(obj, \"save\");\n",
        "save",
        &[(1, R)],
    ),
    ("cpp", "store.save(d);\n", "save", &[]),
    // ---- C# / Kotlin / Swift / Dart / PHP ----------------------------------
    (
        "csharp",
        "var m = typeof(T).GetMethod(\"Save\");\n",
        "Save",
        &[(1, R)],
    ),
    ("csharp", "button.Click += Save;\n", "Save", &[(1, F)]),
    ("csharp", "public void Save() {}\n", "Save", &[]),
    ("kotlin", "list.forEach(::save)\n", "save", &[(1, F)]),
    ("kotlin", "fun save(d: Doc) {}\n", "save", &[]),
    (
        "swift",
        "b.addTarget(self, action: #selector(save), for: .tap)\n",
        "save",
        &[(1, F)],
    ),
    ("swift", "func save() {}\n", "save", &[]),
    (
        "dart",
        "ElevatedButton(onPressed: save, child: x)\n",
        "save",
        &[(1, F)],
    ),
    ("dart", "void save() {}\n", "save", &[]),
    (
        "php",
        "call_user_func([$this, 'save']);\n",
        "save",
        &[(1, R)],
    ),
    (
        "php",
        "if (method_exists($obj, 'save')) {}\n",
        "save",
        &[(1, R)],
    ),
    ("php", "$map = ['save' => 'onSave'];\n", "save", &[(1, K)]),
    ("php", "$save = 1;\n", "save", &[]),
    ("php", "# call_user_func('save');\n", "save", &[]),
    ("php", "$this->save($d);\n", "save", &[]),
    // ---- a local binding of the name shadows bare references in its file ----
    // Found by the hono/express survey: `url`, `method`, `type` are functions
    // there AND locals/parameters, and every bare use read as a reference.
    // Qualified references (`obj.method`) are still reported.
    (
        "javascript",
        "function f(url) { return fetch(url); }\n",
        "url",
        &[],
    ),
    (
        "javascript",
        "const type = x;\nreturn { type: type };\n",
        "type",
        &[],
    ),
    (
        "javascript",
        "const method = pick();\nlog(method);\nreg(obj.method);\n",
        "method",
        &[(3, F)],
    ),
    (
        "typescript",
        "app.get('/', (c, url) => fetch(url));\n",
        "url",
        &[],
    ),
    (
        "javascript",
        "class A { m(url) {\n  fetch(url);\n} }\n",
        "url",
        &[],
    ),
    ("python", "def f(url):\n    fetch(url)\n", "url", &[]),
    ("python", "url = get()\nfetch(url)\n", "url", &[]),
    ("python", "for url in urls:\n    fetch(url)\n", "url", &[]),
    ("go", "url := get()\nfetch(url)\n", "url", &[]),
    ("rust", "let url = get();\nfetch(url);\n", "url", &[]),
    ("rust", "xs.map(|url| fetch(url));\n", "url", &[]),
    (
        "java",
        "void f(String url) {\n  fetch(url);\n}\n",
        "url",
        &[],
    ),
    (
        "javascript",
        "function run() { register(save); }\n",
        "save",
        &[(1, F)],
    ),
    (
        "javascript",
        "if (save) { x(); }\nregister(save);\n",
        "save",
        &[(2, F)],
    ),
    // A default parameter VALUE is a reference, not the parameter (found on
    // this repo: `function uninstall({ install = npmInstallGlobal } = {})`).
    (
        "javascript",
        "function f({ install = save } = {}) {}\n",
        "save",
        &[(1, F)],
    ),
    (
        "javascript",
        "function g(run = save) {}\n",
        "save",
        &[(1, F)],
    ),
    ("python", "def g(run=save):\n    pass\n", "save", &[(1, F)]),
    (
        "javascript",
        "const { run = save } = opts;\n",
        "save",
        &[(1, F)],
    ),
    // Not bindings: a bitwise `|`, and a brace-less condition's call.
    (
        "javascript",
        "x = a | save;\nregister(save);\n",
        "save",
        &[(2, F)],
    ),
    (
        "rust",
        "if check(save) {\n}\nregister(save);\n",
        "save",
        &[(1, F), (3, F)],
    ),
    (
        "go",
        "if check(Save) {\n}\nregister(Save)\n",
        "Save",
        &[(1, F), (3, F)],
    ),
    // ---- languages with no shape table: never report -------------------------
    ("markdown", "handlers[\"save\"]\n", "save", &[]),
    ("bash", "trap save EXIT\n", "save", &[]),
];

#[test]
fn every_corpus_row_reports_exactly_its_expected_sites() {
    let mut failures = Vec::new();
    for (i, (lang, src, name, want)) in CORPUS.iter().enumerate() {
        let got: Vec<(usize, Shape)> = scan_source(lang, src, name)
            .into_iter()
            .map(|h| (h.line, h.shape))
            .collect();
        if got.as_slice() != *want {
            failures.push(format!(
                "row {i} [{lang}] {src:?} name={name}: want {want:?}, got {got:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} corpus rows wrong:\n{}",
        failures.len(),
        CORPUS.len(),
        failures.join("\n")
    );
}

/// Non-vacuity: the corpus must exercise every shape and both verdicts, so a
/// scanner that returns nothing (or one shape for everything) cannot pass.
#[test]
fn corpus_covers_every_shape_and_every_language_group() {
    for shape in [R, E, K, S, F] {
        assert!(
            CORPUS.iter().any(|r| r.3.iter().any(|(_, s)| *s == shape)),
            "no accepted row for {shape:?}"
        );
    }
    for lang in [
        "javascript",
        "typescript",
        "python",
        "ruby",
        "go",
        "rust",
        "java",
        "c",
        "cpp",
        "csharp",
        "kotlin",
        "swift",
        "dart",
        "php",
    ] {
        let rows: Vec<_> = CORPUS.iter().filter(|r| r.0 == lang).collect();
        assert!(!rows.is_empty(), "no rows for {lang}");
        assert!(
            rows.iter().any(|r| !r.3.is_empty()),
            "{lang}: no accepted row"
        );
        assert!(
            rows.iter().any(|r| r.3.is_empty()),
            "{lang}: no look-alike row"
        );
    }
}

/// The `via` detail names the dispatching callee for the two call-shaped kinds.
#[test]
fn reflection_and_event_hits_name_their_callee() {
    let h = scan_source("python", "getattr(obj, \"save\")()\n", "save");
    assert_eq!(h[0].via.as_deref(), Some("getattr"));
    let h = scan_source("javascript", "bus.on(\"save\", f);\n", "save");
    assert_eq!(h[0].via.as_deref(), Some("on"));
    let h = scan_source("javascript", "app.post(\"/x\", save);\n", "save");
    assert_eq!(h[0].via, None);
}

/// A definition written as a binding (`const save = () => …`) is the
/// function, not a shadowing local: with its line passed as a definition, the
/// file's bare references still report.
#[test]
fn a_binding_on_a_definition_line_does_not_shadow() {
    let src = "const save = () => 1;\napp.post(\"/\", save);\n";
    assert!(scan_source("javascript", src, "save").is_empty());
    let hits = scan_source_with_defs("javascript", src, "save", &[1]);
    assert_eq!(
        hits.iter().map(|h| (h.line, h.shape)).collect::<Vec<_>>(),
        vec![(2, F)]
    );
}

/// One site per line: a line carrying several shapes reports the most specific.
#[test]
fn one_line_reports_one_site() {
    let h = scan_source(
        "python",
        "D = {\"save\": save}; getattr(x, \"save\")\n",
        "save",
    );
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].shape, R);
}
