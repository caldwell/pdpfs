// Copyright © 2023 David Caldwell <david@porkrind.org>

import globals from "globals";
import js from "@eslint/js";

// The react plugin doesn't appear to support their new module interface
// yet. It should just be:
//   import react from "eslint-plugin-react-hooks";
import { FlatCompat } from "@eslint/eslintrc";
import path from "path";
import { fileURLToPath } from "url";
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const compat = new FlatCompat({
    baseDirectory: __dirname
});

export default [
    js.configs.recommended,
    ...compat.extends("plugin:react-hooks/recommended"),
    {
        languageOptions: {
            parserOptions: {
                sourceType: "module",
                ecmaVersion: "latest",
            },
            globals: {
                ...globals.browser,
                ...globals.node,
                pdpfs: "readonly",
            },
        },
        rules: {
            "no-undef": "error",
            "no-unused-vars": ["warn", { varsIgnorePattern: "^_",
                                         argsIgnorePattern: "^_",
                                         destructuredArrayIgnorePattern: "^_",
                                         caughtErrorsIgnorePattern: "^_" }],
            "react-hooks/exhaustive-deps": "error",
        },
    }
];
