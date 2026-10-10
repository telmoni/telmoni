//! Auth's outbound mail — the copy, and only the copy.
//!
//! What an identity provider knows nothing about: the two deletion codes, the
//! email-change code, the invitation, and an ownership offer, its withdrawal
//! and its answer. **The password reset is the provider's**: one press
//! producing two credential-bearing mails from two senders left a reader
//! unable to tell which was real, so the provider mails it alone. When the
//! provider is the built-in one, that is the last four methods here, and it
//! is still the only sender.
//!
//! ⚠ **The email-change code is the near miss.** The provider mails a code on
//! the same press, but to the NEW address, proving it is reachable; ours goes
//! to the CURRENT address, proving the asker owns the account. Neither alone
//! finishes the change, so there is nothing for a reader to mistake.
//!
//! None of these belongs in the notifications queue. A code must report its
//! delivery honestly, and a queue keyed on the account's identity would follow
//! the very change it is gating; an invitee may have no feed at all. An
//! ownership mail is for one person, and an organization's feed is everyone's
//! in it.

use std::sync::Arc;

use async_trait::async_trait;
use telmoni_shared::mail::{Mail, MailSender};
use telmoni_shared::{OrganizationRole, Role};

pub use telmoni_shared::mail::MailError;

/// Which roster an invitation seats somebody on, at which role there. The two
/// ladders spell their roles the same, so the role alone cannot say what an
/// admin may do; the level can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitedTo {
    /// A seat on one project (`auth.project_members`).
    Project(Role),
    /// A row on the organization's roster (`auth.organization_members`).
    Organization(OrganizationRole),
}

/// The mail auth sends directly.
#[async_trait]
pub trait Mailer: Send + Sync {
    /// Send the code that authorizes deleting ONE organization, which the mail
    /// names: the irreversible cascade. Never logged.
    async fn send_organization_deletion_code(
        &self,
        to: &str,
        code: &str,
        organization: &str,
    ) -> Result<(), MailError>;

    /// Send the code that authorizes deleting the person's account. Never
    /// logged.
    async fn send_account_deletion_code(&self, to: &str, code: &str) -> Result<(), MailError>;

    /// Send an email-change confirmation code to the account's CURRENT address.
    async fn send_email_change_code(
        &self,
        to: &str,
        code: &str,
        new_email: &str,
    ) -> Result<(), MailError>;

    /// Invite somebody to an organization or one of its projects, at the role
    /// `level` names on it.
    async fn send_member_invite(
        &self,
        to: &str,
        inviter: &str,
        organization: &str,
        level: InvitedTo,
        link: &str,
    ) -> Result<(), MailError>;

    /// Tell an admin their organization's owner has offered them the
    /// organization. Nothing changes until they accept in the console.
    async fn send_ownership_offer(
        &self,
        to: &str,
        owner: &str,
        organization: &str,
        link: &str,
    ) -> Result<(), MailError>;

    /// Tell an admin the offer they held is gone: its owner withdrew it, or
    /// offered the organization to somebody else. Nothing changed for them.
    async fn send_ownership_offer_withdrawn(
        &self,
        to: &str,
        owner: &str,
        organization: &str,
    ) -> Result<(), MailError>;

    /// Tell the previous owner the admin they offered the organization to has
    /// accepted it. The console tells them only if a tab is open.
    async fn send_ownership_accepted(
        &self,
        to: &str,
        new_owner: &str,
        organization: &str,
    ) -> Result<(), MailError>;

    /// Tell the owner the admin they offered the organization to has declined
    /// it. Nothing changed.
    async fn send_ownership_declined(
        &self,
        to: &str,
        admin: &str,
        organization: &str,
    ) -> Result<(), MailError>;

    /// Tell a project admin its owner has offered them the project, which
    /// would move into an organization of theirs. Nothing changes until they
    /// accept in the console.
    async fn send_project_offer(
        &self,
        to: &str,
        owner: &str,
        project: &str,
        organization: &str,
        link: &str,
    ) -> Result<(), MailError>;

    /// Tell an admin the offer of a project they held is gone: its owner
    /// withdrew it, or offered the project to somebody else.
    async fn send_project_offer_withdrawn(
        &self,
        to: &str,
        owner: &str,
        project: &str,
    ) -> Result<(), MailError>;

    /// Tell the previous owner the admin they offered a project to has taken
    /// it, and into which organization.
    async fn send_project_accepted(
        &self,
        to: &str,
        new_owner: &str,
        project: &str,
        organization: &str,
    ) -> Result<(), MailError>;

    /// Tell the owner the admin they offered a project to has declined it.
    /// Nothing changed.
    async fn send_project_declined(
        &self,
        to: &str,
        admin: &str,
        project: &str,
    ) -> Result<(), MailError>;

    /// The built-in provider's first mail: the link that proves a new
    /// account's address is the person's.
    async fn send_email_verification(&self, to: &str, link: &str) -> Result<(), MailError>;

    /// The built-in provider's reset: the link a new password is chosen from.
    async fn send_password_reset(&self, to: &str, link: &str) -> Result<(), MailError>;

    /// The code the built-in provider mails to the NEW address of a change,
    /// which proves that address is reachable; [`Mailer::send_email_change_code`]
    /// goes to the current one.
    async fn send_new_address_code(&self, to: &str, code: &str) -> Result<(), MailError>;
}

/// Writes auth's mails and hands each to a transport.
pub struct ComposingMailer {
    sender: Arc<dyn MailSender>,
    /// The deployment's support address, for the line every mail ends on:
    /// where a reader the mail was not meant for turns.
    support_email: Option<String>,
}

impl ComposingMailer {
    /// Wire the copy to a transport.
    #[must_use]
    pub fn new(sender: Arc<dyn MailSender>) -> Self {
        Self {
            sender,
            support_email: None,
        }
    }

    /// Name the address a reader is told to write to (`SUPPORT_EMAIL`).
    #[must_use]
    pub fn with_support_email(mut self, support_email: Option<String>) -> Self {
        self.support_email = support_email.filter(|s| !s.trim().is_empty());
        self
    }

    /// Where an unexpecting reader is sent. A deployment that configured no
    /// address is still told where to turn, in words, rather than handed a
    /// blank.
    fn contact(&self) -> &str {
        self.support_email
            .as_deref()
            .unwrap_or("whoever runs this service")
    }

    async fn send(&self, to: &str, subject: String, text: String) -> Result<(), MailError> {
        self.sender
            .send(&Mail {
                to: to.to_owned(),
                subject,
                text,
                html: None,
                from: None,
                list_unsubscribe: None,
            })
            .await
    }
}

#[async_trait]
impl Mailer for ComposingMailer {
    async fn send_organization_deletion_code(
        &self,
        to: &str,
        code: &str,
        organization: &str,
    ) -> Result<(), MailError> {
        self.send(
            to,
            format!(
                "Confirm deleting the {organization} organization on {}",
                telmoni_shared::PRODUCT_NAME
            ),
            format!(
                "Your organization-deletion confirmation code is {code}.\n\n\
                 Enter it to permanently delete the organization {organization}: \
                 its projects and API keys, for everyone in it. Your \
                 own account is not deleted. This is immediate and irreversible — \
                 there is no recovery window.\n\n\
                 The code expires in 15 minutes. If you didn't request this, ignore \
                 this email and the organization stays safe. If you think someone \
                 else has access to your account, write to {contact} right away.",
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_account_deletion_code(&self, to: &str, code: &str) -> Result<(), MailError> {
        self.send(
            to,
            format!(
                "Confirm deleting your {} account",
                telmoni_shared::PRODUCT_NAME
            ),
            format!(
                "Your account-deletion confirmation code is {code}.\n\n\
                 Enter it to permanently delete your account: your sign-in, and \
                 every organization you own on your own. You leave every other \
                 organization you are in, and it keeps everything. This is \
                 immediate and irreversible — there is no recovery window.\n\n\
                 The code expires in 15 minutes. If you didn't request this, ignore \
                 this email and your account stays safe. If you think someone else \
                 has access to your account, write to {contact} right away.",
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_email_change_code(
        &self,
        to: &str,
        code: &str,
        new_email: &str,
    ) -> Result<(), MailError> {
        self.send(
            to,
            format!(
                "Confirm the new email address on your {} account",
                telmoni_shared::PRODUCT_NAME
            ),
            format!(
                "Your confirmation code is {code}.\n\n\
                 Somebody asked to change this account's email address to \
                 {new_email}. Enter this code, together with the code we sent \
                 to that address, to finish the change. Both are needed, so \
                 nothing has changed yet.\n\n\
                 The code expires in 15 minutes. If you didn't request this, \
                 ignore this email and your address stays as it is. If you \
                 think someone else has access to your account, write to \
                 {contact} right away — this email is the address that would \
                 stop working.",
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_member_invite(
        &self,
        to: &str,
        inviter: &str,
        organization: &str,
        level: InvitedTo,
        link: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        let role = match level {
            InvitedTo::Project(role) => role.to_string(),
            InvitedTo::Organization(role) => role.to_string(),
        };
        self.send(
            to,
            format!("{inviter} invited you to {organization} on {product}"),
            format!(
                "{inviter} invited you to {organization} on {product} as {role}.\n\n\
                 {product} is a platform for organizations and projects, \
                 with who may see and change what decided per project. \
                 Accepting lets you collaborate on theirs — nothing of yours \
                 is shared with them, and you can leave at any time. {can}\n\n\
                 Accept here:\n{link}\n\n\
                 The invitation expires in {days} days, and the link works \
                 once. You will be asked to sign in with this address — \
                 signing up with it is enough if you have no account yet.\n\n\
                 If you do not know {inviter}, do nothing and the \
                 invitation expires. To tell us it was unwanted, write to \
                 {contact}.",
                can = match level {
                    InvitedTo::Project(Role::Member) => {
                        "A member has read-only access to the project: its members, API keys and connectors."
                    }
                    InvitedTo::Project(Role::Admin) => {
                        "An admin manages the project: its members, API keys and connectors, with its audit log to read; only the organization's owner can delete or hand over the project."
                    }
                    InvitedTo::Organization(OrganizationRole::Admin) => {
                        "An admin can create projects, manage the organization's members and settings, and work in every project as an admin; only its owner can delete the organization or its projects, or hand either over."
                    }
                    InvitedTo::Organization(OrganizationRole::Member) => {
                        "A member sees the projects they are given access to, and nothing else."
                    }
                    // No invitation makes an owner, who is made only by a
                    // handover they accept; were one sent, it would claim no
                    // access it cannot describe.
                    InvitedTo::Project(Role::Owner)
                    | InvitedTo::Organization(OrganizationRole::Owner) => {
                        "Your access would be whatever that role allows."
                    }
                },
                days = crate::handler::invite::INVITE_TTL_DAYS,
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_ownership_offer(
        &self,
        to: &str,
        owner: &str,
        organization: &str,
        link: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{owner} wants to make you the owner of {organization}"),
            format!(
                "{owner} has offered to hand you the {product} organization \
                 {organization}. Nothing has changed yet: it becomes yours only \
                 when you accept.\n\n\
                 As the owner you could do what only an owner can: delete the \
                 organization, hand it on, and delete its projects or hand them \
                 to other organizations. {owner} would stay on as an admin.\n\n\
                 Accept or decline here:\n{link}\n\n\
                 The offer lapses in {days} days. If you did not expect it, do \
                 nothing, or decline it. To tell us it was unwanted, write to \
                 {contact}.",
                days = crate::db::organization_members::OFFER_TTL_DAYS,
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_ownership_offer_withdrawn(
        &self,
        to: &str,
        owner: &str,
        organization: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{owner} withdrew the offer of {organization}"),
            format!(
                "{owner} withdrew their offer to hand you the {product} \
                 organization {organization}. Nothing has changed: you are still \
                 one of its admins, and there is nothing for you to do."
            ),
        )
        .await
    }

    async fn send_ownership_accepted(
        &self,
        to: &str,
        new_owner: &str,
        organization: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{new_owner} is now the owner of {organization}"),
            format!(
                "{new_owner} accepted your offer and now owns the {product} \
                 organization {organization}.\n\n\
                 You stay on as an admin, which makes you an admin in every one \
                 of its projects: you can go on managing its members, API keys \
                 and connectors, but deleting the organization or its projects, \
                 and handing either on, are {new_owner}'s to decide now. You can \
                 leave the organization from its Overview page at any time.\n\n\
                 If you did not offer them the organization, write to {contact} \
                 right away.",
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_ownership_declined(
        &self,
        to: &str,
        admin: &str,
        organization: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{admin} declined to take over {organization}"),
            format!(
                "{admin} declined your offer to hand them the {product} \
                 organization {organization}. Nothing has changed: it is still \
                 yours, with its API keys and members.\n\n\
                 To hand it to somebody else, offer it to another admin from the \
                 organization's Members page."
            ),
        )
        .await
    }

    async fn send_project_offer(
        &self,
        to: &str,
        owner: &str,
        project: &str,
        organization: &str,
        link: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{owner} wants to hand you the project {project}"),
            format!(
                "{owner} has offered to hand you the {product} project {project}, \
                 which is in the organization {organization}. Nothing has changed \
                 yet: it becomes yours only when you accept.\n\n\
                 Accepting moves it into an organization you own. Its members \
                 come with it and join that organization as members, and so do \
                 its API keys — rotate any key you did not mint yourself. Its \
                 Slack, Discord and webhook connectors do not: they belong to \
                 {organization}, and you would connect your own. {owner} stays \
                 on as an admin of the project, and joins your organization as \
                 a member.\n\n\
                 Accept or decline here:\n{link}\n\n\
                 The offer lapses in {days} days. If you did not expect it, do \
                 nothing, or decline it. To tell us it was unwanted, write to \
                 {contact}.",
                days = crate::db::members::OFFER_TTL_DAYS,
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_project_offer_withdrawn(
        &self,
        to: &str,
        owner: &str,
        project: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{owner} withdrew the offer of the project {project}"),
            format!(
                "{owner} withdrew their offer to hand you the {product} project \
                 {project}. Nothing has changed: you are still one of its admins, \
                 and there is nothing for you to do."
            ),
        )
        .await
    }

    async fn send_project_accepted(
        &self,
        to: &str,
        new_owner: &str,
        project: &str,
        organization: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{new_owner} now owns the project {project}"),
            format!(
                "{new_owner} accepted your offer, and the {product} project \
                 {project} is now in their organization {organization}.\n\n\
                 You stay on as an admin of the project, and as a member of \
                 {organization}: you can go on managing the project's members, \
                 API keys and connectors and reading its audit log, but deleting \
                 it or handing it on is {new_owner}'s to decide now. Its API keys went \
                 with it, and its connectors did not — \
                 they stayed with your organization and were disconnected. You \
                 can leave the project from its Members page, or the \
                 organization from its Overview, at any time.\n\n\
                 If you did not offer them the project, write to {contact} right \
                 away.",
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_project_declined(
        &self,
        to: &str,
        admin: &str,
        project: &str,
    ) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("{admin} declined to take over the project {project}"),
            format!(
                "{admin} declined your offer to hand them the {product} project \
                 {project}. Nothing has changed: it is still yours, with its \
                 members, API keys and connectors.\n\n\
                 To hand it to somebody else, offer it to another admin from the \
                 project's Members page."
            ),
        )
        .await
    }

    async fn send_email_verification(&self, to: &str, link: &str) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("Confirm your email address for {product}"),
            format!(
                "Confirm this is your address to finish creating your {product} \
                 account:\n{link}\n\n\
                 The link works once and expires in {hours} hours. If you didn't \
                 create an account, ignore this email: nothing happens, and the \
                 account cannot be used without this confirmation.",
                hours = crate::password::VERIFICATION_TTL_HOURS,
            ),
        )
        .await
    }

    async fn send_password_reset(&self, to: &str, link: &str) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("Reset your {product} password"),
            format!(
                "Somebody asked to reset the password on your {product} account. \
                 Choose a new one here:\n{link}\n\n\
                 The link works once and expires in {minutes} minutes, and every \
                 signed-in session ends when you use it. If you didn't ask, ignore \
                 this email and your password stays as it is. If you think someone \
                 else has access to your account, write to {contact} right away.",
                minutes = crate::password::RESET_TTL_MINUTES,
                contact = self.contact(),
            ),
        )
        .await
    }

    async fn send_new_address_code(&self, to: &str, code: &str) -> Result<(), MailError> {
        let product = telmoni_shared::PRODUCT_NAME;
        self.send(
            to,
            format!("Confirm this email address for your {product} account"),
            format!(
                "Your confirmation code is {code}.\n\n\
                 Somebody asked to make this the address on their {product} \
                 account. Enter this code, together with the code we sent to the \
                 account's current address, to finish the change. Both are \
                 needed, so nothing has changed yet.\n\n\
                 The code expires in 15 minutes. If you didn't ask for this, \
                 ignore this email and nothing changes."
            ),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposingMailer, InvitedTo, Mailer};
    use std::sync::{Arc, Mutex};
    use telmoni_shared::mail::{Mail, MailError, MailSender};
    use telmoni_shared::{OrganizationRole, Role};

    /// Captures what the copy handed to the transport.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Mail>>);

    #[async_trait::async_trait]
    impl MailSender for Recorder {
        async fn send(&self, mail: &Mail) -> Result<(), MailError> {
            self.0.lock().expect("recorder lock").push(mail.clone());
            Ok(())
        }
    }

    /// The deployment's support address, as a suite configures it.
    const SUPPORT: &str = "support@example.com";

    async fn composed(send: impl AsyncFnOnce(ComposingMailer)) -> Mail {
        let recorder = Arc::new(Recorder::default());
        send(ComposingMailer::new(recorder.clone()).with_support_email(Some(SUPPORT.into()))).await;
        let sent = recorder.0.lock().expect("recorder lock");
        sent.first().expect("one mail was composed").clone()
    }

    /// A deployment that configured no address still tells an unexpecting
    /// reader where to turn, and prints no blank where the address would be.
    #[tokio::test]
    async fn without_a_support_address_the_reader_is_still_told_where_to_turn() {
        let recorder = Arc::new(Recorder::default());
        ComposingMailer::new(recorder.clone())
            .send_account_deletion_code("ada@example.com", "311406")
            .await
            .expect("recorded");
        let mail = recorder.0.lock().expect("recorder lock")[0].clone();
        assert!(
            mail.text.contains("write to whoever runs this service"),
            "{}",
            mail.text
        );
        assert!(
            !mail.text.contains("write to  "),
            "a blank address: {}",
            mail.text
        );
    }

    #[tokio::test]
    async fn the_deletion_mails_tell_an_unauthorized_reader_where_to_go() {
        let organization = composed(async |m| {
            m.send_organization_deletion_code("ada@example.com", "802610", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        let account = composed(async |m| {
            m.send_account_deletion_code("ada@example.com", "311406")
                .await
                .expect("recorded");
        })
        .await;

        for (mail, code) in [(&organization, "802610"), (&account, "311406")] {
            assert!(
                mail.text.contains(SUPPORT),
                "a deletion mail names no contact address: {}",
                mail.text
            );
            assert!(mail.text.contains(code), "{}", mail.text);
        }
        assert!(
            organization.text.contains("Acme"),
            "the organization's deletion code does not say which organization: {}",
            organization.text
        );
    }

    /// The subject is what a person reads before deciding whether what is
    /// below it is theirs to act on, so no two mails share one.
    #[tokio::test]
    async fn each_mail_names_what_it_is_about() {
        let organization_deletion = composed(async |m| {
            m.send_organization_deletion_code("ada@example.com", "802610", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        let account_deletion = composed(async |m| {
            m.send_account_deletion_code("ada@example.com", "311406")
                .await
                .expect("recorded");
        })
        .await;
        let email_change = composed(async |m| {
            m.send_email_change_code("ada@example.com", "119284", "ada.new@example.com")
                .await
                .expect("recorded");
        })
        .await;
        let invite = composed(async |m| {
            m.send_member_invite(
                "ada@example.com",
                "Grace Hopper",
                "Acme",
                InvitedTo::Project(Role::Admin),
                "https://telmoni.com/invite/x",
            )
            .await
            .expect("recorded");
        })
        .await;
        let offer = composed(async |m| {
            m.send_ownership_offer(
                "ada@example.com",
                "Grace Hopper",
                "Acme",
                "https://telmoni.com/account/notifications",
            )
            .await
            .expect("recorded");
        })
        .await;
        let withdrawn = composed(async |m| {
            m.send_ownership_offer_withdrawn("ada@example.com", "Grace Hopper", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        let accepted = composed(async |m| {
            m.send_ownership_accepted("grace@example.com", "Ada Lovelace", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        let declined = composed(async |m| {
            m.send_ownership_declined("grace@example.com", "Ada Lovelace", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        let project_offer = composed(async |m| {
            m.send_project_offer(
                "ada@example.com",
                "Grace Hopper",
                "Payments",
                "Acme",
                "https://telmoni.com/account/notifications",
            )
            .await
            .expect("recorded");
        })
        .await;
        let project_withdrawn = composed(async |m| {
            m.send_project_offer_withdrawn("ada@example.com", "Grace Hopper", "Payments")
                .await
                .expect("recorded");
        })
        .await;
        let project_accepted = composed(async |m| {
            m.send_project_accepted("grace@example.com", "Ada Lovelace", "Payments", "Babbage")
                .await
                .expect("recorded");
        })
        .await;
        let project_declined = composed(async |m| {
            m.send_project_declined("grace@example.com", "Ada Lovelace", "Payments")
                .await
                .expect("recorded");
        })
        .await;

        // Each prints its `subject`, never the `Mail`: its `Debug` masks the
        // subject, which can carry a name.
        assert!(
            organization_deletion.subject.contains("organization"),
            "{}",
            organization_deletion.subject
        );
        assert!(
            account_deletion.subject.contains("account"),
            "{}",
            account_deletion.subject
        );
        assert!(
            email_change.subject.contains("email address"),
            "{}",
            email_change.subject
        );
        assert!(
            invite.subject.contains("Grace Hopper"),
            "{}",
            invite.subject
        );
        assert!(offer.subject.contains("owner"), "{}", offer.subject);
        assert!(
            withdrawn.subject.contains("withdrew") && withdrawn.subject.contains("Grace Hopper"),
            "{}",
            withdrawn.subject
        );
        assert!(
            accepted.subject.contains("owner") && accepted.subject.contains("Ada Lovelace"),
            "{}",
            accepted.subject
        );
        assert!(
            declined.subject.contains("declined") && declined.subject.contains("Ada Lovelace"),
            "{}",
            declined.subject
        );

        // The project mails name the project, so a person offered both an
        // organization and one of its projects can tell the two apart.
        for mail in [
            &project_offer,
            &project_withdrawn,
            &project_accepted,
            &project_declined,
        ] {
            assert!(
                mail.subject.contains("project Payments"),
                "{}",
                mail.subject
            );
        }

        let subjects = [
            &organization_deletion.subject,
            &account_deletion.subject,
            &email_change.subject,
            &invite.subject,
            &offer.subject,
            &withdrawn.subject,
            &accepted.subject,
            &declined.subject,
            &project_offer.subject,
            &project_withdrawn.subject,
            &project_accepted.subject,
            &project_declined.subject,
        ];
        for (i, a) in subjects.iter().enumerate() {
            for b in subjects.iter().skip(i + 1) {
                assert_ne!(a, b, "two mails share a subject");
            }
        }
        let lowered = email_change.subject.to_lowercase();
        assert!(
            !lowered.contains("delete") && !lowered.contains("deletion"),
            "the email-change code wears a deletion mail's subject, and these \
             are the mails a reader must never confuse: {}",
            email_change.subject
        );
        assert!(
            !organization_deletion.subject.contains("account")
                && !account_deletion.subject.contains("organization"),
            "the two deletion codes wear each other's subject: {} / {}",
            organization_deletion.subject,
            account_deletion.subject
        );
    }

    /// The mail goes to the address that is LEAVING, so it is the one warning a
    /// person gets that somebody is moving their account: it names the
    /// destination and gives them somewhere to push back.
    #[tokio::test]
    async fn the_email_change_code_names_where_the_account_would_go() {
        let mail = composed(async |m| {
            m.send_email_change_code("ada@example.com", "119284", "ada.new@example.com")
                .await
                .expect("recorded");
        })
        .await;

        assert!(mail.text.contains("119284"), "{}", mail.text);
        assert!(
            mail.text.contains("ada.new@example.com"),
            "the mail does not say where the account is being moved to: {}",
            mail.text
        );
        assert!(
            mail.text.contains(SUPPORT),
            "the mail names no contact address: {}",
            mail.text
        );
        assert!(
            mail.text.contains("nothing has changed yet"),
            "the mail does not say the change has not happened: {}",
            mail.text
        );
        assert_eq!(mail.to, "ada@example.com");
    }

    /// The mail has one job beyond breaking the news: make the next step
    /// obvious — the count, the page, and somewhere to push back.
    #[tokio::test]
    async fn the_invitation_carries_its_link_and_says_what_the_role_can_do() {
        let mail = composed(async |m| {
            m.send_member_invite(
                "ada@example.com",
                "grace@example.com",
                "Acme",
                InvitedTo::Project(Role::Member),
                "https://telmoni.com/invite/tok",
            )
            .await
            .expect("recorded");
        })
        .await;

        assert!(
            mail.text.contains("https://telmoni.com/invite/tok"),
            "carries no link: {}",
            mail.text
        );
        assert!(
            mail.text.contains("read-only access to the project"),
            "does not say what a member can do: {}",
            mail.text
        );
        assert!(
            mail.text
                .contains("is a platform for organizations and projects"),
            "does not say what the product is: {}",
            mail.text
        );
        assert!(
            mail.text.contains("do nothing"),
            "does not tell an unexpecting reader they can ignore it: {}",
            mail.text
        );
        assert!(
            mail.text.contains(SUPPORT),
            "no way to report an unwanted invitation: {}",
            mail.text
        );
    }

    /// A stranger's inbox is not the place to learn our vocabulary. The mail
    /// says who is asking, and into what, in the words the console shows.
    #[tokio::test]
    async fn the_invitation_names_who_is_asking_and_where() {
        let mail = composed(async |m| {
            m.send_member_invite(
                "ada@example.com",
                "Grace Hopper",
                "Acme",
                InvitedTo::Project(Role::Admin),
                "https://telmoni.com/invite/tok",
            )
            .await
            .expect("recorded");
        })
        .await;
        assert!(mail.text.contains("Grace Hopper"), "{}", mail.text);
        assert!(mail.text.contains("Acme"), "{}", mail.text);
    }

    /// Each role on either level is named as the console names it and reads
    /// its own sentence, word for word. An owner, whom no invitation makes,
    /// reads the vague one rather than a sentence claiming access it cannot
    /// describe.
    #[tokio::test]
    async fn every_level_and_role_is_described_in_its_own_words() {
        let vague = "Your access would be whatever that role allows.";
        let pinned = [
            (InvitedTo::Project(Role::Owner), "owner", vague),
            (
                InvitedTo::Project(Role::Admin),
                "admin",
                "An admin manages the project: its members, API keys and connectors, with its \
                 audit log to read; only the organization's owner can delete or hand over the \
                 project.",
            ),
            (
                InvitedTo::Project(Role::Member),
                "member",
                "A member has read-only access to the project: its members, API keys and \
                 connectors.",
            ),
            (
                InvitedTo::Organization(OrganizationRole::Owner),
                "owner",
                vague,
            ),
            (
                InvitedTo::Organization(OrganizationRole::Admin),
                "admin",
                "An admin can create projects, manage the organization's members and settings, \
                 and work in every project as an admin; only its owner can delete the \
                 organization or its projects, or hand either over.",
            ),
            (
                InvitedTo::Organization(OrganizationRole::Member),
                "member",
                "A member sees the projects they are given access to, and nothing else.",
            ),
        ];
        assert_eq!(
            pinned.len(),
            Role::all().len() + OrganizationRole::all().len(),
            "a role on one of the ladders has no sentence pinned here"
        );
        for (level, role, sentence) in pinned {
            let mail = composed(async |m| {
                m.send_member_invite(
                    "ada@example.com",
                    "grace@example.com",
                    "Acme",
                    level,
                    "https://telmoni.com/invite/tok",
                )
                .await
                .expect("recorded");
            })
            .await;
            assert!(
                mail.text.starts_with(&format!(
                    "grace@example.com invited you to Acme on {} as {role}.\n\n",
                    telmoni_shared::PRODUCT_NAME
                )),
                "{level:?}: {}",
                mail.text
            );
            assert!(mail.text.contains(sentence), "{level:?}: {}", mail.text);
        }
    }

    /// The offer changes nothing by itself, and says so — and it names what
    /// the reader would hold.
    #[tokio::test]
    async fn the_ownership_offer_says_nothing_has_changed_and_what_it_would_hand_over() {
        let mail = composed(async |m| {
            m.send_ownership_offer(
                "ada@example.com",
                "Grace Hopper",
                "Acme",
                "https://telmoni.com/account/notifications",
            )
            .await
            .expect("recorded");
        })
        .await;
        assert!(
            mail.text.contains("Nothing has changed yet"),
            "{}",
            mail.text
        );
        assert!(mail.text.contains("Acme"), "{}", mail.text);
        assert!(
            mail.text.contains(
                "delete the organization, hand it on, and delete its projects or hand them to \
                 other organizations"
            ),
            "{}",
            mail.text
        );
        assert!(
            mail.text
                .contains("https://telmoni.com/account/notifications"),
            "{}",
            mail.text
        );
        assert!(mail.text.contains(SUPPORT), "{}", mail.text);
    }

    /// The previous owner learns what they kept — an admin's place in every
    /// project, managing as before — and what only the new owner decides now.
    #[tokio::test]
    async fn the_accepted_offer_says_what_the_previous_owner_kept_and_what_moved() {
        let mail = composed(async |m| {
            m.send_ownership_accepted("grace@example.com", "Ada Lovelace", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        assert_eq!(mail.to, "grace@example.com");
        assert!(mail.text.contains("Acme"), "{}", mail.text);
        assert!(
            mail.text.contains("admin in every one"),
            "does not say what the previous owner can still do: {}",
            mail.text
        );
        assert!(
            mail.text.contains("Ada Lovelace's to decide"),
            "does not say who decides now: {}",
            mail.text
        );
        assert!(
            mail.text.contains(SUPPORT),
            "no way to report a handover nobody offered: {}",
            mail.text
        );
    }

    /// A withdrawn offer changed nothing for the admin who held it, and says
    /// so, without sending them looking for an offer that is gone.
    #[tokio::test]
    async fn the_withdrawn_offer_says_nothing_has_changed() {
        let mail = composed(async |m| {
            m.send_ownership_offer_withdrawn("ada@example.com", "Grace Hopper", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        assert_eq!(mail.to, "ada@example.com");
        assert!(mail.text.contains("Acme"), "{}", mail.text);
        assert!(mail.text.contains("Nothing has changed"), "{}", mail.text);
        assert!(mail.text.contains("nothing for you to do"), "{}", mail.text);
    }

    /// The project offer says what a handover moves and what it leaves: the
    /// keys come, so the reader is told to rotate them; the connectors stay,
    /// so the reader is not left looking for them.
    #[tokio::test]
    async fn the_project_offer_says_what_comes_with_it_and_what_stays_behind() {
        let mail = composed(async |m| {
            m.send_project_offer(
                "ada@example.com",
                "Grace Hopper",
                "Payments",
                "Acme",
                "https://telmoni.com/account/notifications",
            )
            .await
            .expect("recorded");
        })
        .await;
        assert_eq!(mail.to, "ada@example.com");
        assert!(mail.text.contains("Nothing has changed"), "{}", mail.text);
        assert!(mail.text.contains("Payments"), "{}", mail.text);
        assert!(mail.text.contains("Acme"), "{}", mail.text);
        assert!(mail.text.contains("rotate any key"), "{}", mail.text);
        assert!(
            mail.text.contains("connectors do not"),
            "does not say the connectors stay behind: {}",
            mail.text
        );
        assert!(
            mail.text
                .contains("https://telmoni.com/account/notifications"),
            "{}",
            mail.text
        );
        assert!(mail.text.contains(SUPPORT), "{}", mail.text);

        let accepted = composed(async |m| {
            m.send_project_accepted("grace@example.com", "Ada Lovelace", "Payments", "Babbage")
                .await
                .expect("recorded");
        })
        .await;
        assert_eq!(accepted.to, "grace@example.com");
        assert!(accepted.text.contains("Babbage"), "{}", accepted.text);
        assert!(
            accepted.text.contains("stay on as an admin"),
            "does not say what the previous owner keeps: {}",
            accepted.text
        );
        assert!(
            accepted.text.contains("were disconnected"),
            "does not say what became of the connectors: {}",
            accepted.text
        );
    }

    /// A declined offer changed nothing, and says so.
    #[tokio::test]
    async fn the_declined_offer_says_nothing_has_changed() {
        let mail = composed(async |m| {
            m.send_ownership_declined("grace@example.com", "Ada Lovelace", "Acme")
                .await
                .expect("recorded");
        })
        .await;
        assert_eq!(mail.to, "grace@example.com");
        assert!(mail.text.contains("Acme"), "{}", mail.text);
        assert!(mail.text.contains("Nothing has changed"), "{}", mail.text);
    }
}
