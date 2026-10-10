FROM node:24

ENV NODE_ENV=production

WORKDIR /app

COPY . .

RUN npm rebuild better-sqlite3

VOLUME ["/root/.cliproam"]

EXPOSE 4810

CMD ["node", "build/index.js"]
