#!/bin/bash
echo_blue() {
    echo -e "\001\033[94m\002$*\001\033[0m\002"
}
echo_green() {
    echo -e "\001\033[93m\002$*\001\033[0m\002"
}

run() {
    echo_blue "+ $*" >&2
    "$@"
}

echo_green "Clearing the git-remote-walrus cache..."
run rm -rf "$HOME"/.cache/git-remote-walrus
echo_green "Displaying current git-remote-walrus config. NB: we are running against localnet..."
run bat "$HOME"/.config/git-remote-walrus/config.yaml
echo_green "Now go run the demo in another terminal window and come back when you have a remote_state object to look at."
read -rp "Enter a remote_state object: " remote_state_object
echo_green "Displaying remote state object $remote_state_object..."
run sui client --client.config "$HOME"/src/walrus/working_dir/sui_client.yaml object "$remote_state_object"
read -rp "What is the objects_blob_object_id: " blob_object
echo_green "Displaying blob object $blob_object."
echo_green "This is the object that contains the mappings from Git commit hashes to Walrus blob 'slices'."
echo_green "Let's look inside it..."
run sui client --client.config "$HOME"/src/walrus/working_dir/sui_client.yaml object "$blob_object" 
read -rp "Let's go deeper. What is the blob_id for the objects blob object? (should be in decimal format): " decimal_blob_id
run walrus --config "$HOME"/src/walrus/working_dir/client_config.yaml convert-blob-id $decimal_blob_id
read -rp "What is the blob_id in base64 format? " base64_blob_id
run walrus --config "$HOME"/src/walrus/working_dir/client_config.yaml read "$base64_blob_id" > /tmp/object_blob.yaml
bat /tmp/object_blob.yaml

