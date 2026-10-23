;; TeaVM's low-level diagnostics run only on fatal VM paths. Fail the callback
;; rather than pretending that a Java fault or exhausted heap succeeded.
(module
  (func (export "logString") (param i32) unreachable)
  (func (export "logInt") (param i32) unreachable)
  (func (export "logOutOfMemory") unreachable)
)
