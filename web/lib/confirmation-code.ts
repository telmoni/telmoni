/** How many digits auth's emailed confirmation codes have
 *  (`confirmation_code_length` in the wire contract). */
export const CONFIRMATION_CODE_LENGTH = 6;

const CODE = new RegExp(`^\\d{${CONFIRMATION_CODE_LENGTH}}$`);

/** What an action answers a code that is not one. */
export const NOT_A_CODE = `Enter the ${CONFIRMATION_CODE_LENGTH}-digit code from your email.`;

/** Whether `code` is one auth could have sent. */
export function isConfirmationCode(code: string): boolean {
  return CODE.test(code);
}

/** What a person typed into a code field, kept to the digits a code holds. */
export function codeTyped(raw: string): string {
  return raw.replace(/\D/g, "").slice(0, CONFIRMATION_CODE_LENGTH);
}
