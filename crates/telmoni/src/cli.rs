//! The command line: one process, one command, then either a listener or an
//! exit. Parsed by hand — six words are not worth a parser crate.

use telmoni_shared::OrganizationId;

/// Which of auth's sweeps to run once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sweep {
    /// Drive every `pending_deletion` organization's tail, then every pending
    /// person's erasure, to completion.
    Deletion,
    /// Walk every organization's tamper-evident audit chain.
    AuditVerify,
    /// Hard-delete what has aged out: revoked keys, stale invitations,
    /// revoked sessions, spent grants.
    Retention,
    /// Embed again every passage of the agent's index that a model other
    /// than `EMBEDDINGS_MODEL` embedded.
    AgentReindex,
}

impl Sweep {
    /// Every sweep, in the order the usage line lists them.
    pub const ALL: [Self; 4] = [
        Self::Deletion,
        Self::AuditVerify,
        Self::Retention,
        Self::AgentReindex,
    ];

    fn parse(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == arg)
    }

    /// The usage line's `<a|b|c>`, built from [`Self::ALL`].
    fn names() -> String {
        Self::ALL
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }

    /// The word on the command line, and in the log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deletion => "deletion",
            Self::AuditVerify => "audit-verify",
            Self::Retention => "retention",
            Self::AgentReindex => "agent-reindex",
        }
    }
}

/// What one invocation does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Listen: every module's routes and loops, until told to stop.
    Serve,
    /// Run every pending migration, then the object grants, and exit.
    Migrate,
    /// Rotate the audit partitions: create the months ahead, drop the
    /// expired where the registry allows it (nowhere today), and exit.
    Rotate,
    /// One sweep, once, and exit.
    Sweep(Sweep),
    /// Close an organization without its owner's code.
    Terminate(OrganizationId),
}

impl Command {
    /// The usage line.
    #[must_use]
    pub fn usage() -> String {
        format!(
            "usage: telmoni serve | migrate | rotate | sweep <{}> | terminate <org_id>",
            Sweep::names()
        )
    }

    /// Parse the arguments after the binary's name.
    pub fn parse(args: &[String]) -> anyhow::Result<Self> {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["serve"] => Ok(Self::Serve),
            ["migrate"] => Ok(Self::Migrate),
            ["rotate"] => Ok(Self::Rotate),
            ["sweep", sweep] => Sweep::parse(sweep)
                .map(Self::Sweep)
                .ok_or_else(|| anyhow::anyhow!("{} (got sweep {sweep:?})", Self::usage())),
            ["terminate", organization] => {
                let id = OrganizationId::try_new(*organization)
                    .map_err(|e| anyhow::anyhow!("{} (bad organization id: {e})", Self::usage()))?;
                Ok(Self::Terminate(id))
            }
            _ => anyhow::bail!("{} (got {args:?})", Self::usage()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> anyhow::Result<Command> {
        Command::parse(&words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn every_command_parses_and_nothing_else_does() {
        assert_eq!(parse(&["serve"]).unwrap(), Command::Serve);
        assert_eq!(parse(&["migrate"]).unwrap(), Command::Migrate);
        assert_eq!(parse(&["rotate"]).unwrap(), Command::Rotate);
        for sweep in Sweep::ALL {
            assert_eq!(
                parse(&["sweep", sweep.as_str()]).unwrap(),
                Command::Sweep(sweep)
            );
        }
        assert!(matches!(
            parse(&["terminate", "org_abc"]).unwrap(),
            Command::Terminate(id) if id.as_str() == "org_abc"
        ));

        for bad in [
            vec![],
            vec!["deletion"],
            vec!["sweep"],
            vec!["sweep", "everything"],
            vec!["serve", "now"],
            vec!["terminate"],
            vec!["terminate", "not an id"],
            vec!["restore", "org_abc"],
        ] {
            let err = parse(&bad).expect_err(&format!("{bad:?} parsed"));
            assert!(err.to_string().starts_with("usage:"), "{err}");
        }
    }
}
