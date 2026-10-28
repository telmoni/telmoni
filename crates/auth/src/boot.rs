//! What the environment says about sign-in, read once at boot: the login
//! form and who may create an account, the
//! OpenID Connect provider beside the form, and the first account. A binary
//! built for another deployment reads its own provider's variables instead
//! and hands it to [`providers_with`].
//!
//! **Sign-in.** Every session is the issuer's, whichever way it began, and
//! its tokens are opaque secrets auth stores as their hashes: there is no
//! signing key to configure. The login form — email and password, held
//! here — is on unless `AUTH_DISABLE_USERNAME_PASSWORD=true`; nobody new may
//! create an account, at the form or at a provider, unless
//! `AUTH_DISABLE_SIGNUP=false`, though an invited address always may; an
//! address under one of `AUTH_DOMAINS_WITH_SSO_ENFORCEMENT` signs in at a
//! provider alone; whether a new account confirms its address first is
//! `VERIFY_EMAIL` (off), and `ADMIN_EMAIL` with `ADMIN_PASSWORD` seeds the
//! first account at boot; every account created joins the organization
//! `TELMONI_DEFAULT_ORG_ID` names, if one, and the project
//! `TELMONI_DEFAULT_PROJECT_ID` names in it ([`crate::default_organization`]).
//! Set `AUTH_CUSTOM_CLIENT_ID`, `AUTH_CUSTOM_ISSUER` and
//! `AUTH_CUSTOM_CLIENT_SECRET`, and the sign-in page offers "Continue with
//! `AUTH_CUSTOM_NAME`" beside the form: the provider's endpoints are
//! discovered from the issuer on first use, and an unknown subject whose
//! address an account here holds is that account only under
//! `AUTH_CUSTOM_ALLOW_ACCOUNT_LINKING`, and only for an address both the
//! provider and the account proved.

use std::sync::Arc;

use telmoni_shared::Redacted;
use telmoni_shared::config::{env_parse, https_on_a_tier, optional, require};
use telmoni_shared::mail::MailSender;
use telmoni_shared::oidc::{Discovery, IdTokenVerifier, KeySetUrl, SigningAlg};

use crate::default_organization::DefaultOrganization;
use crate::external::External;
use crate::issuer::Issuer;
use crate::mailer::ComposingMailer;
use crate::oidc::{OidcProvider, Settings};
use crate::password::PasswordProvider;
use crate::{Config, Providers};

/// The issuer every session is minted by, the login form unless it is off,
/// and the external provider when its block names one — all from the
/// environment.
pub async fn providers_from_env(
    config: &Config,
    http: &reqwest::Client,
    pool: sqlx::PgPool,
    mail: Arc<dyn MailSender>,
) -> anyhow::Result<Providers> {
    let external = match optional("AUTH_CUSTOM_CLIENT_ID") {
        Some(client_id) => Some(external_from_env(http, client_id)?),
        None => {
            // A provider is on when its client id is set; the rest of a
            // block without it is a mistake, said before anybody signs in.
            if let Some(name) = ["AUTH_CUSTOM_ISSUER", "AUTH_CUSTOM_CLIENT_SECRET"]
                .into_iter()
                .find(|name| optional(name).is_some())
            {
                anyhow::bail!(
                    "{name} is set without AUTH_CUSTOM_CLIENT_ID: a provider is on only when its \
                     client id is set"
                );
            }
            None
        }
    };
    providers_with(config, pool, mail, external).await
}

/// `AUTH_DISABLE_SIGNUP`: whether nobody new may create an account, at the
/// form or at a provider — `true` unless set, so a deployment reachable from
/// the internet registers nobody until its operator opens it.
fn sign_ups_closed() -> anyhow::Result<bool> {
    env_parse("AUTH_DISABLE_SIGNUP", true)
}

/// `AUTH_DOMAINS_WITH_SSO_ENFORCEMENT`: the domains whose addresses sign in
/// at a provider alone, comma-separated.
fn sso_domains() -> Vec<String> {
    optional("AUTH_DOMAINS_WITH_SSO_ENFORCEMENT")
        .map(|list| held_domains(&list))
        .unwrap_or_default()
}

/// The domains a list names, lowercased as an address is
/// (`Email::parse`), so one typed in capitals still matches.
fn held_domains(list: &str) -> Vec<String> {
    list.split(',')
        .map(|domain| domain.trim().trim_start_matches('@').to_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect()
}

/// [`providers_from_env`] with the external provider the binary chose in
/// place of the one the `CUSTOM` block would name: the issuer and the login
/// form still come from the environment, and the same rules hold — the form
/// may be off only beside a provider, and so may a domain be held to one.
pub async fn providers_with(
    config: &Config,
    pool: sqlx::PgPool,
    mail: Arc<dyn MailSender>,
    external: Option<External>,
) -> anyhow::Result<Providers> {
    let verify_email: bool = env_parse("VERIFY_EMAIL", false)?;
    let default_organization = DefaultOrganization::from_env()?;
    if let Some(default_organization) = &default_organization {
        default_organization.check(&pool).await?;
    }
    let issuer = Arc::new(
        Issuer::new(pool.clone(), &config.app_url, verify_email)
            .with_default_organization(default_organization),
    );

    let sso_domains = sso_domains();
    if !sso_domains.is_empty() && external.is_none() {
        anyhow::bail!(
            "AUTH_DOMAINS_WITH_SSO_ENFORCEMENT is set and no external provider is configured: \
             nobody at those domains could sign in"
        );
    }
    let disable_login_form: bool = env_parse("AUTH_DISABLE_USERNAME_PASSWORD", false)?;
    let password = if disable_login_form {
        if external.is_none() {
            anyhow::bail!(
                "AUTH_DISABLE_USERNAME_PASSWORD is set and no external provider is configured: \
                 nobody could sign in"
            );
        }
        if optional("ADMIN_EMAIL").is_some() || optional("ADMIN_PASSWORD").is_some() {
            anyhow::bail!(
                "ADMIN_EMAIL is set with AUTH_DISABLE_USERNAME_PASSWORD: an account with a \
                 password could never sign in"
            );
        }
        None
    } else {
        let mailer = ComposingMailer::new(Arc::clone(&mail))
            .with_support_email(config.support_email.clone());
        let password = Arc::new(PasswordProvider::new(
            pool,
            Arc::clone(&issuer),
            Arc::new(mailer),
            !sign_ups_closed()?,
            sso_domains,
        ));
        seed_admin_from_env(&password).await?;
        Some(password)
    };

    tracing::info!(
        login_form = password.is_some(),
        sign_ups_open = password.as_ref().is_some_and(|p| p.allow_sign_up()),
        verify_email,
        external_provider = external.is_some(),
        default_organization = issuer.default_organization().is_some(),
        "sign-in configured"
    );
    Ok(Providers {
        issuer,
        password,
        external,
        mail,
    })
}

/// `ADMIN_EMAIL` and `ADMIN_PASSWORD`: the first account, created once.
async fn seed_admin_from_env(password: &PasswordProvider) -> anyhow::Result<()> {
    match (optional("ADMIN_EMAIL"), optional("ADMIN_PASSWORD")) {
        (None, None) => Ok(()),
        (Some(email), Some(secret)) => {
            let email = telmoni_shared::Email::parse(&email)
                .map_err(|e| anyhow::anyhow!("ADMIN_EMAIL: {e}"))?;
            if password.held_to_sso(&email) {
                anyhow::bail!(
                    "ADMIN_EMAIL is under AUTH_DOMAINS_WITH_SSO_ENFORCEMENT: its password could \
                     never sign in"
                );
            }
            let secret = Redacted::from(secret);
            let created = password
                .seed_admin(email.as_str(), secret.expose())
                .await
                .map_err(|e| anyhow::anyhow!("ADMIN_EMAIL/ADMIN_PASSWORD: {e}"))?;
            if created {
                tracing::info!("admin account created from ADMIN_EMAIL");
            } else {
                tracing::info!("admin account present; ADMIN_PASSWORD left as it was");
            }
            Ok(())
        }
        _ => anyhow::bail!("ADMIN_EMAIL and ADMIN_PASSWORD go together; set both or neither"),
    }
}

/// The `CUSTOM` block's OpenID Connect provider, beside the accounts held
/// here, as a confidential client.
fn external_from_env(http: &reqwest::Client, client_id: String) -> anyhow::Result<External> {
    let issuer_url = https_on_a_tier("AUTH_CUSTOM_ISSUER", require("AUTH_CUSTOM_ISSUER")?)?;
    let client_secret: Redacted = require("AUTH_CUSTOM_CLIENT_SECRET")?.into();
    let name = optional("AUTH_CUSTOM_NAME").unwrap_or_else(|| "single sign-on".to_owned());
    let scope = optional("AUTH_CUSTOM_SCOPE").unwrap_or_else(|| "openid email profile".to_owned());
    if !scope.split_whitespace().any(|s| s == "openid") {
        anyhow::bail!(
            "AUTH_CUSTOM_SCOPE leaves out openid: the provider would answer with no id token, \
             and nobody could be named"
        );
    }

    let alg: SigningAlg = optional("AUTH_CUSTOM_ID_TOKEN_SIGNED_RESPONSE_ALG")
        .map(|name| name.parse())
        .transpose()
        .map_err(|why| anyhow::anyhow!("AUTH_CUSTOM_ID_TOKEN_SIGNED_RESPONSE_ALG: {why}"))?
        .unwrap_or_default();
    let discovery = Arc::new(Discovery::new(http.clone(), &issuer_url));
    let id_token_verifier = Arc::new(
        IdTokenVerifier::new(
            http.clone(),
            KeySetUrl::Discovered(Arc::clone(&discovery)),
            &client_id,
            &issuer_url,
        )
        .with_alg(alg),
    );
    let provider = OidcProvider::new(
        http.clone(),
        Settings {
            client_id,
            client_secret,
            issuer: issuer_url,
            name,
            scope,
            fetch_userinfo: env_parse("AUTH_CUSTOM_FETCH_USERINFO", false)?,
            checks: optional("AUTH_CUSTOM_CHECKS")
                .map(|list| list.parse())
                .transpose()
                .map_err(|why| anyhow::anyhow!("AUTH_CUSTOM_CHECKS: {why}"))?
                .unwrap_or_default(),
        },
        discovery,
        id_token_verifier,
    );
    Ok(External {
        provider: Arc::new(provider),
        allow_sign_up: !sign_ups_closed()?,
        link_by_email: env_parse("AUTH_CUSTOM_ALLOW_ACCOUNT_LINKING", false)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A held domain is read as an address's domain is, however it was typed.
    #[test]
    fn a_held_domain_is_read_as_an_address_is() {
        assert_eq!(
            held_domains(" Corp.Example, @MÜNCHEN.example ,,"),
            ["corp.example", "münchen.example"]
        );
    }
}
