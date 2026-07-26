#!/bin/bash
API_KEY="${GROQ_API_KEY:-}"

if [ -z "$API_KEY" ]; then
    echo "ERROR: GROQ_API_KEY not set"
    exit 1
fi

curl -s -X POST "https://api.groq.com/openai/v1/chat/completions" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3-70b-8192",
    "messages": [{"role": "user", "content": "hello"}],
    "max_tokens": 10
  }' | jq '.'
