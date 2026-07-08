declare module "nspell" {
	interface Nspell {
		correct(word: string): boolean;
		suggest(word: string): string[];
		spell(word: string): { correct: boolean };
		add(word: string): this;
		remove(word: string): this;
		wordCharacters(): string | null;
		dictionary(buf: Buffer | string): this;
		personal(buf: Buffer | string): this;
	}

	function nspell(aff: Buffer | string, dic: Buffer | string): Nspell;

	export default nspell;
}
