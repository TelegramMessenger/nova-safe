// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Times engines that each evaluate a script from source against engines that
//! evaluate one CompiledScript of it.
//!
//! `cargo run --release -p nova_embed --example compiled_script_bench -- <script.js> [engines]`

use std::time::{Duration, Instant};

use nova_embed::{CompiledScript, Engine, EngineOptions, Host, HostArg, HostReturn};
use nova_vm::{
    ecmascript::{
        AgentOptions, CompiledScript as VmCompiledScript, DefaultHostHooks, GcAgent,
        String as JsString, parse_script,
    },
    engine::Bindable,
};

struct NoHost;

impl Host for NoHost {
    fn call(&self, _id: u32, _args: Vec<HostArg>) -> HostReturn {
        HostReturn::Undefined
    }
}

fn new_engine() -> Engine {
    Engine::new(
        EngineOptions {
            memory_limit: 0,
            stack_limit: 0,
        },
        Box::new(NoHost),
    )
    .expect("engine")
}

fn new_agent() -> (GcAgent, nova_vm::ecmascript::RealmRoot) {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();
    (agent, realm)
}

/// Times `CompiledScript::instantiate` alone, each in a fresh agent.
fn instantiate_only(source: &str, agents: u32) {
    let (mut agent, realm) = new_agent();
    let compiled = agent.run_in_realm(&realm, |agent, gc| {
        let source = JsString::from_str(agent, source, gc.nogc());
        let realm = agent.current_realm(gc.nogc());
        let script = parse_script(agent, source, realm, false, None, gc.nogc()).unwrap();
        VmCompiledScript::export(agent, script, gc.nogc())
            .ok()
            .expect("exports")
    });
    let mut total = Duration::ZERO;
    for _ in 0..agents {
        let (mut agent, realm) = new_agent();
        agent.run_in_realm(&realm, |agent, gc| {
            let realm = agent.current_realm(gc.nogc());
            let started = Instant::now();
            let script = compiled.instantiate(agent, realm, None, gc.nogc());
            total += started.elapsed();
            let _ = script.unbind();
        });
    }
    println!(
        "  instantiate only:             {:.2} ms per agent",
        total.as_secs_f64() * 1000.0 / f64::from(agents)
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a script path");
    let engines: u32 = args.next().map_or(20, |n| n.parse().expect("a count"));
    let source = std::fs::read_to_string(&path).expect("readable script");
    if std::env::var_os("INSTANTIATE_ONLY").is_some() {
        instantiate_only(&source, engines);
        return;
    }

    let started = Instant::now();
    let compiled = CompiledScript::compile(&source).expect("compiles");
    let compile_time = started.elapsed();

    let mut create = Duration::ZERO;
    let mut from_source = Duration::ZERO;
    let mut from_compiled = Duration::ZERO;
    for _ in 0..engines {
        let started = Instant::now();
        let mut engine = new_engine();
        create += started.elapsed();
        let started = Instant::now();
        let parsed = engine.evaluate(&source, false);
        from_source += started.elapsed();
        drop(engine);

        let mut engine = new_engine();
        let started = Instant::now();
        let instantiated = engine.evaluate_compiled(&compiled);
        from_compiled += started.elapsed();
        assert_eq!(parsed, instantiated);
    }
    let per = |total: Duration| total.as_secs_f64() * 1000.0 / f64::from(engines);
    println!("{path}: {} bytes, {engines} engines", source.len());
    println!(
        "  CompiledScript::compile once: {:.2} ms",
        compile_time.as_secs_f64() * 1000.0
    );
    println!(
        "  Engine::new:                  {:.2} ms per engine",
        per(create)
    );
    println!(
        "  evaluate(source):             {:.2} ms per engine",
        per(from_source)
    );
    println!(
        "  evaluate_compiled:            {:.2} ms per engine",
        per(from_compiled)
    );
}
