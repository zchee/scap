pub fn run(args: &crate::cli::RootArgs) -> anyhow::Result<()> {
    let roots = crate::config::resolve_roots(args.all)?;
    if args.all {
        for root in roots {
            println!("{}", root.display());
        }
    } else if let Some(first) = roots.into_iter().next() {
        println!("{}", first.display());
    }
    Ok(())
}
