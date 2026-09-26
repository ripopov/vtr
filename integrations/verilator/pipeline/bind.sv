// Attaches the tracer to every demo_core; the port expressions resolve inside
// demo_core, so the design is not edited.
bind demo_core demo_tracer u_vtr (.*);
