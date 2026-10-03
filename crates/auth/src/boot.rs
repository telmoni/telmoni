//! What the environment says about sign-in, read once at boot: the login
//! form and who may create an account, the OpenID Connect provider beside
//! the form, and the first account. A binary built for another deployment
//! reads its own provider's variables instead and hands it to
//! [`providers_with`].
//!
//! **Sign-in.** Every session is the issuer's, whichever way it began, and
//! its tokens are opaque secrets auth stores as their hashes: there is no
//! signing key to configure. The login form — email and password, held
//! here — is on unless `DISABLE_LOGIN_FORM=true`; who may create an account
//! with a password is `ALLOW_SIGN_UP` (off: invited addresses alone; the
//! provider's own sign-ups are `OIDC_ALLOW_SIGN_UP`'s), whether a new account
//! confirms its address first is `VERIFY_EMAIL` (off), and `ADMIN_EMAIL`
//! with `ADMIN_PASSWORD` seeds the first account at boot. Set `OIDC_ISSUER`,
//! `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET`, and the sign-in page offers
//! "Continue with `OIDC_NAME`" beside the form: the provider's endpoints are
//! discovered from the issuer on first use, a subject it names for the first
//! time becomes an account here while `OIDC_ALLOW_SIGN_UP` (on) allows, and
//! one whose address an account here already holds is that account only
//! under `OIDC_ALLOW_INSECURE_EMAIL_LOOKUP`.

use std::sync::Arc;

use telmoni_shared::Redacted;
use telmoni_shared::config::{env_parse, optional, require};
use telmoni_shared::envelope::on_deployed_tier;
use telmoni_shared::mail::MailSender;
use telmoni_shared::oidc::{Discovery, IdTokenVerifier, KeySetUrl};

use crate::external::External;
use crate::issuer::Issuer;
use crate::mailer::ComposingMailer;
use crate::oidc::OidcProvider;
use crate::password::PasswordProvider;
use crate::{Config, Providers};

/// The issuer every session is minted by, the login form unless it is off,
/// and the external provider when one is named — all from the environment.
pub async fn providers_from_env(
    config: &Config,
    http: &reqwest::Client,
    pool: sqlx::PgPool,
    mail: Arc<dyn MailSender>,
) -> anyhow::Result<Providers> {
    let external = match optional("OIDC_ISSUER") {
        Some(issuer_url) => Some(external_from_env(http, issuer_url)?),
        None => None,
    };
    providers_with(config, pool, mail, external).await
}

/// [`providers_from_env`] with the external provider the binary chose in
/// place of the one `OIDC_ISSUER` would name: the issuer and the login form
/// still come from the environment, and the same rules hold — the form may
/// be off only beside a provider.
pub async fn providers_with(
    config: &Config,
    pool: sqlx::PgPool,
    mail: Arc<dyn MailSender>,
    external: Option<External>,
) -> anyhow::Result<Providers> {
    let verify_email: bool = env_parse("VERIFY_EMAIL", false)?;
    let issuer = Arc::new(Issuer::new(pool.clone(), &config.app_url, verify_email));

    let disable_login_form: bool = env_parse("DISABLE_LOGIN_FORM", false)?;
    let password = if disable_login_form {
        if external.is_none() {
            anyhow::bail!(
                "DISABLE_LOGIN_FORM is set and no external provider is configured: nobody could \
                 sign in"
            );
        }
        if optional("ADMIN_EMAIL").is_some() || optional("ADMIN_PASSWORD").is_some() {
            anyhow::bail!(
                "ADMIN_EMAIL is set with DISABLE_LOGIN_FORM: an account with a password could \
                 never sign in"
            );
        }
        None
    } else {
        let allow_sign_up: bool = env_parse("ALLOW_SIGN_UP", false)?;
        let mailer = ComposingMailer::new(Arc::clone(&mail))
            .with_support_email(config.support_email.clone());
        let password = Arc::new(PasswordProvider::new(
            pool,
            Arc::clone(&issuer),
            Arc::new(mailer),
            allow_sign_up,
        ));
        seed_admin_from_env(&password).await?;
        Some(password)
    };

    tracing::info!(
        login_form = password.is_some(),
        sign_ups_open = password.as_ref().is_some_and(|p| p.allow_sign_up()),
        verify_email,
        external_provider = external.is_some(),
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
            let secret = Redacted::from(secret);
            let created = password
                .seed_admin(&email, secret.expose())
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

/// An OpenID Connect provider beside the accounts held here, as a
/// confidential client.
fn external_from_env(http: &reqwest::Client, issuer_url: String) -> anyhow::Result<External> {
    let client_id = require("OIDC_CLIENT_ID")?;
    let client_secret: Redacted = require("OIDC_CLIENT_SECRET")?.into();
    let name = optional("OIDC_NAME").unwrap_or_else(|| "single sign-on".to_owned());

    if on_deployed_tier() && !issuer_url.starts_with("https://") {
        anyhow::bail!(
            "OIDC_ISSUER is not https in a Kubernetes pod; the provider's key set and every \
             grant would travel in the clear"
        );
    }

    let discovery = Arc::new(Discovery::new(http.clone(), &issuer_url));
    let id_token_verifier = Arc::new(IdTokenVerifier::new(
        http.clone(),
        KeySetUrl::Discovered(Arc::clone(&discovery)),
        &client_id,
        &issuer_url,
    ));
    let provider = OidcProvider::new(
        http.clone(),
        &client_id,
        client_secret,
        &issuer_url,
        &name,
        discovery,
        id_token_verifier,
    );
    Ok(External {
        provider: Arc::new(provider),
        allow_sign_up: env_parse("OIDC_ALLOW_SIGN_UP", true)?,
        link_by_email: env_parse("OIDC_ALLOW_INSECURE_EMAIL_LOOKUP", false)?,
    })
}
