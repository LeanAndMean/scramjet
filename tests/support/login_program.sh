#!/bin/sh
# A package's login program, for scripts/screens/login.json: it signs in after a pause, renews
# nothing, and lists one model at the scripted server (SCREENS_BASE_URL).
read -r request
case "$request" in
*'"action":"login"'*)
    echo '{"open":"https://sign-in.example/authorize?client=screens"}'
    sleep 1
    echo '{"say":"Waiting for the browser sign-in…"}'
    sleep 2
    echo '{"credential":{"access":"screens-token","refresh":"screens","expires":99999999999999}}'
    ;;
*'"action":"refresh"'*)
    echo '{"credential":{"access":"screens-token","expires":99999999999999}}'
    ;;
*'"action":"models"'*)
    echo '{"say":"Reading the models…"}'
    sleep 1
    printf '{"provider":{"baseUrl":"%s","api":"openai-responses","models":[{"id":"acme-large","name":"Acme Large","reasoning":true}]}}\n' "$SCREENS_BASE_URL"
    ;;
*)
    echo '{"error":"unknown action"}'
    ;;
esac
