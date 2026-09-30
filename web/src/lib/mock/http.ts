/** An HTTP error answered by the mock backend as `{error: {code, message}}`. */
export class HttpError extends Error {
	constructor(
		public status: number,
		public code: string,
		message: string
	) {
		super(message);
	}
}
