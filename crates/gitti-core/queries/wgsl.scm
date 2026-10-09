; Minimal WGSL highlights for tree-sitter-wgsl-bevy (the crate ships no queries).

(line_comment) @comment
(block_comment) @comment

(int_literal) @number
(float_literal) @number
(bool_literal) @boolean

(type_declaration) @type
(struct_declaration name: (identifier) @type)

(function_declaration name: (identifier) @function)
(type_constructor_or_function_call_expression
  (type_declaration) @function)

(struct_member (variable_identifier_declaration name: (identifier) @property))
(parameter (variable_identifier_declaration name: (identifier) @variable))

(attribute (identifier) @attribute)
"@" @attribute

[
  "fn" "let" "var" "override" "struct" "return" "if" "else" "for"
  "while" "loop" "break" "continue" "continuing" "switch" "case" "default"
  "discard" "enable" "fallthrough"
] @keyword

[
  "function" "private" "workgroup" "uniform" "storage" "read" "write" "read_write"
] @keyword

[
  "-" "--" "-=" "->" "!" "!=" "*" "*=" "/" "/=" "&" "&&" "&=" "%" "%=" "^" "^="
  "+" "++" "+=" "<" "<<" "<=" "=" "==" ">" ">=" ">>" "|" "|=" "||" "~"
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";" ":" "." "::"] @punctuation.delimiter

(preproc_ifdef) @preproc
(preproc_else) @preproc
(preproc_import) @preproc
(define_import_path) @preproc
