.section .text.default_exc_handler, "ax", @progbits
.align 2
.global default_exc_handler
.type default_exc_handler, @function
default_exc_handler:
    tail __ws63_rf_exception_diag
.size default_exc_handler, . - default_exc_handler
